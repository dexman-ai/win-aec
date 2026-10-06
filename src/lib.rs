//! Windows WASAPI **communications mode** helpers.
//!
//! This crate enables the OS-level voice capture pipeline (acoustic echo
//! cancellation / Voice Clarity) for a WASAPI capture stream by:
//!
//! 1. Setting [`AudioClientProperties::eCategory`] to `AudioCategory_Communications`
//!    via `IAudioClient2::SetClientProperties`, **before** `IAudioClient::Initialize`.
//! 2. Querying `IAcousticEchoCancellationControl` via `IAudioClient::GetService`,
//!    **after** `Initialize`, and calling `SetEchoCancellationRenderEndpoint` to
//!    explicitly bind the capture stream to a specific playback endpoint.
//!
//! `IAcousticEchoCancellationControl` is not present in the generated `windows`
//! crate metadata, so it is declared here against the Windows SDK definition in
//! `audioclient.h` (IID `f4ae25b5-aaa3-437d-b6b3-dbbe2d0e9549`).

use std::mem::size_of;

use windows::Win32::Foundation::{CloseHandle, E_NOINTERFACE, HANDLE, WAIT_EVENT};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, AudioCategory_Communications, AudioClientProperties,
    AUDCLNT_SHAREMODE_SHARED, IAudioCaptureClient, IAudioClient, IAudioClient2,
    IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
use windows::core::{Interface, Result, PCWSTR};

const WAIT_OBJECT_0: u32 = 0x00000000;
const WAIT_TIMEOUT: u32 = 0x00000102;

use crate::bindings::IAcousticEchoCancellationControl;

mod bindings {
    #![allow(non_snake_case)]

    use windows::core::{HRESULT, PCWSTR};
    use windows_core::{IUnknown, IUnknown_Vtbl};

    /// Controls which render endpoint is used as the AEC reference stream.
    ///
    /// Mirrors `IAcousticEchoCancellationControl` from `audioclient.h`.
    /// Available on Windows build 22621 and later.
    #[windows_core::interface("f4ae25b5-aaa3-437d-b6b3-dbbe2d0e9549")]
    pub unsafe trait IAcousticEchoCancellationControl: IUnknown {
        /// Sets the render endpoint used as the AEC reference stream.
        ///
        /// A null `endpoint_id` lets Windows pick the loopback reference device.
        pub unsafe fn SetEchoCancellationRenderEndpoint(
            &self,
            endpoint_id: PCWSTR,
        ) -> HRESULT;
    }
}

struct MixFormat(*mut WAVEFORMATEX);

impl MixFormat {
    fn new(sample_rate: u32, channels: u16, bits_per_sample: u16) -> Self {
        let block_align = ((channels as u32 * bits_per_sample as u32) / 8) as u16;
        let format = Box::new(WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: channels,
            nSamplesPerSec: sample_rate,
            nAvgBytesPerSec: sample_rate * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: bits_per_sample,
            cbSize: 0,
        });
        Self(Box::into_raw(format))
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe {
            let _ = Box::from_raw(self.0);
        }
    }
}

/// A WASAPI capture stream configured for communications-mode audio input.
pub struct AudioInputStream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    mix_format: MixFormat,
    echo_cancellation_endpoint_bound: bool,
    event: HANDLE,
}

impl Drop for AudioInputStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
            CoUninitialize();
        }
    }
}

impl AudioInputStream {
    pub fn new(
        mic_id: Option<&str>,
        reference_output_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
    ) -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };

        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let device = match mic_id {
            Some(id) => {
                let wide = to_wide_null(id);
                unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr()))? }
            }
            None => unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)? },
        };
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };

        let client2: IAudioClient2 = client.cast()?;
        let properties = AudioClientProperties {
            cbSize: size_of::<AudioClientProperties>() as u32,
            eCategory: AudioCategory_Communications,
            ..Default::default()
        };
        unsafe { client2.SetClientProperties(&properties)? };

        let mix_format = MixFormat::new(sample_rate, channels, bits_per_sample);
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                200_000,
                0,
                mix_format.0,
                None,
            )?
        };

        let echo_cancellation_endpoint_bound = match unsafe {
            client.GetService::<IAcousticEchoCancellationControl>()
        } {
            Ok(control) => {
                match reference_output_id {
                    Some(id) => {
                        let wide = to_wide_null(id);
                        unsafe {
                            control
                                .SetEchoCancellationRenderEndpoint(PCWSTR(wide.as_ptr()))
                                .ok()?
                        };
                    }
                    None => unsafe {
                        control
                            .SetEchoCancellationRenderEndpoint(PCWSTR::null())
                            .ok()?
                    },
                }
                true
            }
            Err(err) if err.code() == E_NOINTERFACE => false,
            Err(err) => return Err(err),
        };

        let capture: IAudioCaptureClient = unsafe { client.GetService()? };
        let event = unsafe { CreateEventW(None, false, false, PCWSTR::null())? };
        unsafe { client.SetEventHandle(event)? };
        unsafe { client.Start()? };

        Ok(Self {
            client,
            capture,
            mix_format,
            echo_cancellation_endpoint_bound,
            event,
        })
    }

    pub fn read(&self, my_buf: &mut [u8], timeout_ms: i32) -> Result<usize> {
        if timeout_ms != 0 {
            let timeout = if timeout_ms < 0 {
                INFINITE
            } else {
                timeout_ms as u32
            };
            let wait_result = unsafe { WaitForSingleObject(self.event, timeout) };
            if wait_result == WAIT_EVENT(WAIT_TIMEOUT) {
                return Ok(0);
            }
            debug_assert_eq!(wait_result, WAIT_EVENT(WAIT_OBJECT_0));
        }

        let bytes_per_frame = unsafe { (*self.mix_format.0).nBlockAlign as usize };
        let mut data = std::ptr::null_mut();
        let mut frames_to_read = 0u32;
        let mut flags = 0u32;
        let mut device_position = 0u64;
        let mut qpc_position = 0u64;

        unsafe {
            self.capture.GetBuffer(
                &mut data,
                &mut frames_to_read,
                &mut flags,
                Some(&mut device_position),
                Some(&mut qpc_position),
            )?;
        }

        if frames_to_read == 0 {
            unsafe { self.capture.ReleaseBuffer(0)? };
            return Ok(0);
        }

        let bytes_needed = frames_to_read as usize * bytes_per_frame;
        let bytes_to_copy = bytes_needed.min(my_buf.len());

        if bytes_to_copy > 0 && !data.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(data, my_buf.as_mut_ptr(), bytes_to_copy);
            }
        }

        unsafe { self.capture.ReleaseBuffer(frames_to_read)? };
        Ok(bytes_to_copy)
    }

    pub fn sample_rate(&self) -> u32 {
        unsafe { (*self.mix_format.0).nSamplesPerSec }
    }

    pub fn channels(&self) -> u16 {
        unsafe { (*self.mix_format.0).nChannels }
    }

    pub fn bits_per_sample(&self) -> u16 {
        unsafe { (*self.mix_format.0).wBitsPerSample }
    }

    pub fn echo_cancellation_endpoint_bound(&self) -> bool {
        self.echo_cancellation_endpoint_bound
    }
}

/// A WASAPI render stream configured for communications-mode audio output.
pub struct AudioOutputStream {
    client: IAudioClient,
    render: IAudioRenderClient,
    mix_format: MixFormat,
}

impl Drop for AudioOutputStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            CoUninitialize();
        }
    }
}

impl AudioOutputStream {
    pub fn new(
        device_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
    ) -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };

        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let device = match device_id {
            Some(id) => {
                let wide = to_wide_null(id);
                unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr()))? }
            }
            None => unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole)? },
        };
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };

        let client2: IAudioClient2 = client.cast()?;
        let properties = AudioClientProperties {
            cbSize: size_of::<AudioClientProperties>() as u32,
            eCategory: AudioCategory_Communications,
            ..Default::default()
        };
        unsafe { client2.SetClientProperties(&properties)? };

        let mix_format = MixFormat::new(sample_rate, channels, bits_per_sample);
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                200_000,
                0,
                mix_format.0,
                None,
            )?
        };

        let render: IAudioRenderClient = unsafe { client.GetService()? };
        unsafe { client.Start()? };

        Ok(Self {
            client,
            render,
            mix_format,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        let bytes_per_frame = unsafe { (*self.mix_format.0).nBlockAlign as usize };
        if bytes_per_frame == 0 {
            return Ok(0);
        }

        let complete_frames = (data.len() / bytes_per_frame) as u32;
        if complete_frames == 0 {
            return Ok(0);
        }

        let buffer = unsafe { self.render.GetBuffer(complete_frames)? };

        let written_bytes = complete_frames as usize * bytes_per_frame;
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buffer, written_bytes);
            self.render.ReleaseBuffer(complete_frames, 0)?;
        }

        Ok(written_bytes)
    }

    pub fn sample_rate(&self) -> u32 {
        unsafe { (*self.mix_format.0).nSamplesPerSec }
    }

    pub fn channels(&self) -> u16 {
        unsafe { (*self.mix_format.0).nChannels }
    }

    pub fn bits_per_sample(&self) -> u16 {
        unsafe { (*self.mix_format.0).wBitsPerSample }
    }
}

fn to_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(feature = "python")]
use pyo3::exceptions::PyOSError;
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
#[pymodule]
fn win_aec(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<AudioInputStreamPy>()?;
    m.add_class::<AudioOutputStreamPy>()?;
    Ok(())
}

#[cfg(feature = "python")]
#[pyclass(unsendable, name = "AudioInputStream")]
pub struct AudioInputStreamPy {
    inner: AudioInputStream,
}

#[cfg(feature = "python")]
#[pymethods]
impl AudioInputStreamPy {
    #[new]
    #[pyo3(signature = (mic_id=None, reference_output_id=None, sample_rate=48_000, channels=2, bits_per_sample=32))]
    fn new(
        mic_id: Option<&str>,
        reference_output_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: AudioInputStream::new(
                mic_id,
                reference_output_id,
                sample_rate,
                channels,
                bits_per_sample,
            )
            .map_err(|err| PyOSError::new_err(err.to_string()))?,
        })
    }

    #[getter]
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    #[getter]
    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    #[getter]
    fn bits_per_sample(&self) -> u16 {
        self.inner.bits_per_sample()
    }

    #[getter]
    fn echo_cancellation_endpoint_bound(&self) -> bool {
        self.inner.echo_cancellation_endpoint_bound()
    }

    #[pyo3(signature = (timeout_ms = 0))]
    fn read(&self, timeout_ms: i32) -> PyResult<Vec<u8>> {
        let mut buffer = vec![0u8; 4096];
        let read_count = self
            .inner
            .read(&mut buffer, timeout_ms)
            .map_err(|err| PyOSError::new_err(err.to_string()))?;
        buffer.truncate(read_count);
        Ok(buffer)
    }
}

#[cfg(feature = "python")]
#[pyclass(unsendable, name = "AudioOutputStream")]
pub struct AudioOutputStreamPy {
    inner: AudioOutputStream,
}

#[cfg(feature = "python")]
#[pymethods]
impl AudioOutputStreamPy {
    #[new]
    #[pyo3(signature = (device_id=None, sample_rate=48_000, channels=2, bits_per_sample=32))]
    fn new(
        device_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: AudioOutputStream::new(device_id, sample_rate, channels, bits_per_sample)
                .map_err(|err| PyOSError::new_err(err.to_string()))?,
        })
    }

    #[getter]
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    #[getter]
    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    #[getter]
    fn bits_per_sample(&self) -> u16 {
        self.inner.bits_per_sample()
    }

    fn write(&self, data: &[u8]) -> PyResult<usize> {
        self.inner
            .write(data)
            .map_err(|err| PyOSError::new_err(err.to_string()))
    }
}


