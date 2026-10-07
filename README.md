# win-aec

Audio capture (and output) with echo cancellation. Windows-only (WASAPI).

It is designed for Windows voice agents, real-time assistive capture, and wake-word/barge-in scenarios where the OS-level communications pipeline is preferred over a custom audio stack.


Simply instantiate `AudioInputStream` and/or `AudioOutputStream` and call their only method (`read` and `write` respectively).

`AudioInputStream` performs the following operations:
- sets the audio client category to `AudioCategory_Communications` to enable built-in echo cancellation
- initializes streams in WASAPI shared mode
- optionally binds the capture path to a reference render endpoint (output) via `IAcousticEchoCancellationControl`. Needed when the output is not the default one. 

## Example

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

- The requested audio format is only a best-effort hint. WASAPI may negotiate a different sample rate, channel count, or bit depth. Callers must check the actual values from `sample_rate()`, `channels()`, and `bits_per_sample()` (or Python `sample_rate`, `channels`, and `dtype`) after construction, adn perform conversion as needed.

- `read(&mut buffer, timeout_ms)` drains all currently queued WASAPI packets into the caller buffer up to the buffer size. If the queue contains more audio than the caller buffer can hold, the function copies as much as fits and sets `dropped_frames` to `True`.

- `write(&buffer, timeout_ms)` copies only as much as fits to the WASAPI buffer, and returns the number of bytes copied. The caller is responsible for calling `write` again with the remainder of the data.

- bytes vs frames: the Rust versions of `read` and `write` return byte counts. The Python bindings are frame-based (`int16` or `int32`) and return frame counts. 

- timeout values: `0` means non-blocking, negative values wait until the event is ready, and positive values wait for that interval. The Rust API uses milliseconds (hence argument is called `timeout_ms`). Python bindings use seconds (and thus argument is called `timeout`).


