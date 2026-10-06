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
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, IAudioCaptureClient, IAudioClient, IAudioClient2,
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

/// Describes the result of a single capture read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadResult {
    /// Number of bytes actually copied into the caller's buffer.
    pub bytes_read: usize,
    /// True when the read dropped frames because WASAPI reported a discontinuity or
    /// because the caller's buffer was smaller than the WASAPI payload.
    pub dropped_frames: bool,
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
    /// Creates a WASAPI communications-mode input stream.
    ///
    /// This configures the default system audio capture endpoint in voice/communications
    /// mode, optionally binds it to a specific render endpoint for AEC, and starts the
    /// capture pipeline.
    ///
    /// # Arguments
    ///
    /// * `mic_id` - Optional device ID for the capture endpoint, for example
    ///   `"{0.0.1.00000000}.{...}"` or `None` to use the system default microphone.
    /// * `reference_output_id` - Optional render endpoint ID used as the AEC reference,
    ///   for example `"{0.0.0.00000000}.{...}"` or `None` to let Windows choose the
    ///   default loopback/render reference.
    /// * `sample_rate` - Requested sample rate in Hz, typically `48000`.
    /// * `channels` - Number of audio channels, typically `1` or `2`.
    /// * `bits_per_sample` - Bit depth per sample, typically `16` or `32`.
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
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
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

    /// Reads audio data from the capture stream into `my_buf`.
    ///
    /// `my_buf` is the caller-owned destination buffer. It is not resized by the
    /// function; the return value reports the number of bytes actually copied and
    /// whether the read dropped frames. A short read because the caller's buffer was
    /// too small is treated as dropped-frame data, because those samples were not
    /// preserved in the caller's buffer.
    ///
    /// `timeout_ms` follows the WASAPI event semantics:
    ///
    /// * `0` - non-blocking: return immediately if no data is available
    /// * `-1` - block until the event is signaled and data is ready
    /// * `> 0` - wait up to that many milliseconds before returning `0`
    ///
    /// For example, `read(&mut buffer, -1)` blocks until audio is available, while
    /// `read(&mut buffer, 0)` is suitable for polling loops.
    pub fn read(&self, my_buf: &mut [u8], timeout_ms: i32) -> Result<ReadResult> {
        if my_buf.is_empty() {
            return Ok(ReadResult {
                bytes_read: 0,
                dropped_frames: false,
            });
        }

        if timeout_ms != 0 {
            let timeout = if timeout_ms < 0 {
                INFINITE
            } else {
                timeout_ms as u32
            };
            let wait_result = unsafe { WaitForSingleObject(self.event, timeout) };
            if wait_result == WAIT_EVENT(WAIT_TIMEOUT) {
                return Ok(ReadResult {
                    bytes_read: 0,
                    dropped_frames: false,
                });
            }
            debug_assert_eq!(wait_result, WAIT_EVENT(WAIT_OBJECT_0));
        }

        let bytes_per_frame = unsafe { (*self.mix_format.0).nBlockAlign as usize };
        let mut total_bytes = 0usize;
        let mut dropped_frames = false;
        let mut out_off = 0usize;

        loop {
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
                break;
            }

            let bytes_needed = frames_to_read as usize * bytes_per_frame;
            let remaining = my_buf.len().saturating_sub(total_bytes);
            let bytes_to_copy = bytes_needed.min(remaining);
            let discontinuity = (flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;
            dropped_frames |= discontinuity || (bytes_to_copy < bytes_needed);

            if bytes_to_copy > 0 && !data.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data,
                        my_buf.as_mut_ptr().add(out_off),
                        bytes_to_copy,
                    );
                }
                out_off += bytes_to_copy;
                total_bytes += bytes_to_copy;
            }

            unsafe { self.capture.ReleaseBuffer(frames_to_read)? };

            if total_bytes >= my_buf.len() || bytes_to_copy < bytes_needed {
                break;
            }
        }

        Ok(ReadResult {
            bytes_read: total_bytes,
            dropped_frames,
        })
    }

    /// Returns the negotiated sample rate in Hz.
    ///
    /// Typical values are `48000` for voice capture.
    pub fn sample_rate(&self) -> u32 {
        unsafe { (*self.mix_format.0).nSamplesPerSec }
    }

    /// Returns the negotiated number of channels.
    ///
    /// Typical values are `1` for mono or `2` for stereo.
    pub fn channels(&self) -> u16 {
        unsafe { (*self.mix_format.0).nChannels }
    }

    /// Returns the negotiated bit depth per sample.
    ///
    /// Typical values are `16` or `32`.
    pub fn bits_per_sample(&self) -> u16 {
        unsafe { (*self.mix_format.0).wBitsPerSample }
    }

    /// Returns the total capture buffer capacity in bytes.
    ///
    /// This is the native WASAPI buffer size in frames multiplied by the per-frame
    /// storage size: `channels * bytes_per_channel * GetBufferSize()`.
    pub fn buffer_size(&self) -> usize {
        let bytes_per_channel = (self.bits_per_sample() as usize / 8).max(1);
        let buffer_frames = unsafe { self.client.GetBufferSize().unwrap_or(0) } as usize;
        self.channels() as usize * bytes_per_channel * buffer_frames
    }

    /// Indicates whether the stream successfully bound an AEC render endpoint.
    ///
    /// This is `true` when `IAcousticEchoCancellationControl` is available and the
    /// requested endpoint was accepted, and `false` when the device does not expose
    /// that interface or the bind operation is unavailable.
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
    /// Creates a WASAPI communications-mode output stream.
    ///
    /// # Arguments
    ///
    /// * `device_id` - Optional device ID for the render endpoint, for example
    ///   `"{0.0.1.00000000}.{...}"` or `None` to use the default playback device.
    /// * `sample_rate` - Requested sample rate in Hz, typically `48000`.
    /// * `channels` - Number of output channels, typically `1` or `2`.
    /// * `bits_per_sample` - Bit depth per sample, typically `16` or `32`.
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
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
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

    /// Writes PCM bytes to the render stream.
    ///
    /// `data` should contain complete PCM samples in the negotiated format, and the
    /// caller is responsible for passing a buffer whose length is a whole number of
    /// frames. For example, 48 kHz stereo 16-bit audio is `2 channels * 2 bytes per
    /// sample = 4` bytes per frame, so a 960-byte buffer represents 240 frames.
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

    /// Returns the negotiated sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        unsafe { (*self.mix_format.0).nSamplesPerSec }
    }

    /// Returns the negotiated number of output channels.
    pub fn channels(&self) -> u16 {
        unsafe { (*self.mix_format.0).nChannels }
    }

    /// Returns the negotiated bit depth per sample.
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
use pyo3::types::PyByteArray;

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
    /// Creates a Python `AudioInputStream`.
    ///
    /// Example values: `mic_id=None`, `reference_output_id=None`, `sample_rate=48000`,
    /// `channels=2`, `bits_per_sample=16`.
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

    /// Returns the negotiated sample rate in Hz, for example `48000`.
    #[getter]
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    /// Returns the negotiated channel count, for example `2` for stereo.
    #[getter]
    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    /// Returns the bit depth per sample, for example `16` or `32`.
    #[getter]
    fn bits_per_sample(&self) -> u16 {
        self.inner.bits_per_sample()
    }

    /// Returns the total native WASAPI capture buffer capacity in bytes.
    ///
    /// This is computed as `channels * bytes_per_channel * GetBufferSize()`.
    #[getter]
    fn buffer_size(&self) -> usize {
        self.inner.buffer_size()
    }

    /// Returns `True` when the stream successfully bound an AEC render endpoint.
    #[getter]
    fn echo_cancellation_endpoint_bound(&self) -> bool {
        self.inner.echo_cancellation_endpoint_bound()
    }

    /// Reads PCM bytes from the input stream into a caller-provided buffer.
    ///
    /// `buffer` must be a mutable `bytearray` large enough to hold the bytes you
    /// want to read. The function fills the buffer in place and returns a tuple:
    /// `(bytes_read, dropped_frames)`.
    ///
    /// `dropped_frames` is `True` when WASAPI reported a discontinuity or when the
    /// caller's buffer was too small to hold the full audio payload.
    ///
    /// Example: `read(bytearray(4096))` blocks until audio is ready,
    /// `read(bytearray(4096), 0)` polls without blocking, and
    /// `read(bytearray(4096), 250)` waits up to 250 milliseconds.
    #[pyo3(signature = (buffer, timeout_ms = -1))]
    fn read<'py>(
        &self,
        _py: Python<'py>,
        buffer: &Bound<'py, PyByteArray>,
        timeout_ms: i32,
    ) -> PyResult<(usize, bool)> {
        let result = {
            let mut writable = unsafe { buffer.as_bytes_mut() };
            self.inner
                .read(&mut writable, timeout_ms)
                .map_err(|err| PyOSError::new_err(err.to_string()))?
        };
        Ok((result.bytes_read, result.dropped_frames))
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
    /// Creates a Python `AudioOutputStream`.
    ///
    /// Example values: `device_id=None`, `sample_rate=48000`, `channels=2`,
    /// `bits_per_sample=16`.
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

    /// Returns the negotiated sample rate in Hz, for example `48000`.
    #[getter]
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    /// Returns the negotiated channel count, for example `2` for stereo.
    #[getter]
    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    /// Returns the bit depth per sample, for example `16` or `32`.
    #[getter]
    fn bits_per_sample(&self) -> u16 {
        self.inner.bits_per_sample()
    }

    /// Writes PCM bytes to the output stream.
    ///
    /// `data` should contain complete PCM samples in the negotiated format. For
    /// example, a stereo 16-bit wave buffer is `2 channels * 2 bytes = 4` bytes per
    /// frame, so the payload should be a whole number of frames.
    fn write(&self, data: &[u8]) -> PyResult<usize> {
        self.inner
            .write(data)
            .map_err(|err| PyOSError::new_err(err.to_string()))
    }
}


