# win-aec

Audio capture (and output) with OS-level Acoustic Echo Cancellation. Windows-only (WASAPI).

Enables agents to hear the user, not their own audio output. Useful in scenarios where offloading AEC to the OS-level communications pipeline is preferred over a user-space audio stack.

Provides Python and Rust access to built‑in AEC in Windows (the same one used by MS products like Teams), with correct WASAPI integration and minimal latency. 

## Usage:
Simply instantiate `AudioInputStream` and/or `AudioOutputStream` and call their only method (`read` and `write` respectively). `AudioInputStream` allows specifying the output renderer to cancel.

### Python:
```python
import win_aec
import audioop

mic = win_aec.AudioInputStream(None, None, 24000, 1, "int16")
print(f"actual format: {mic.sample_rate} Hz, {mic.channels} ch, {mic.dtype}") # The requested format is a best-effort hint only. 


buf = bytearray(mic.buffer_size)
for _ in range(10):
    frame_count, dropped = mic.read(buf)
    print(f"read {frame_count} frames, dropped_frames={dropped}")
    # use e.g. audioop package to convert buf to desired format
    
```

### Rust:
```rust
use win_aec::AudioInputStream;

fn main() -> windows::core::Result<()> {
    let capture = AudioInputStream::new(None, None, 48_000, 2, 32)?;
    println!(
        "capture format: {} Hz, {} ch, {} bit",
        capture.sample_rate(),
        capture.channels(),
        capture.bits_per_sample()
    );

    let mut buffer = vec![0u8; 4096];
    for _ in 0..10 {
        let res = capture.read(&mut buffer, -1)?;
        println!("read {res.bytes_read} bytes");
    }

    Ok(())
}
```

## Notes

- The requested audio format is only a best-effort hint. WASAPI may negotiate a different sample rate, channel count, or bit depth. Callers must check the actual values from `sample_rate()`, `channels()`, and `bits_per_sample()` (or Python `sample_rate`, `channels`, and `dtype`) after construction, and perform conversion as needed.

- `read(&mut buffer, timeout_ms)` drains all currently queued WASAPI packets into the caller buffer up to the buffer size. If the queue contains more audio than the caller buffer can hold, the function copies as much as fits and sets `dropped_frames` to `True`.

- `write(&buffer, timeout_ms)` copies only as much as fits to the WASAPI buffer, and returns the number of bytes copied. The caller is responsible for calling `write` again with the remainder of the data.

- bytes vs frames: the Rust versions of `read` and `write` return byte counts. The Python bindings are frame-based (`int16` or `int32`) and return frame counts. 

- timeout values: `0` means non-blocking, negative values wait until the event is ready, and positive values wait for that interval. The Rust API uses milliseconds (hence argument is called `timeout_ms`). Python bindings use seconds (and thus argument is called `timeout`).


