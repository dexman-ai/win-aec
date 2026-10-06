# win-aec

Audio capture (and output) with echo cancellation. Windows-only (WASAPI).

It is designed for Windows voice agents, real-time assistive capture, and wake-word/barge-in scenarios where the OS-level communications pipeline is preferred over a custom audio stack.


Simply instantiate `AudioInputStream` and/or `AudioOutputStream` and call their only method (read and write respectively).

`AudioInputStream` performs the following operations:
- sets the audio client category to `AudioCategory_Communications` to enable built-in echo cancellation
- initializes streams in WASAPI shared mode
- optionally binds the capture path to a reference render endpoint (output) via `IAcousticEchoCancellationControl`. This allows echo cancellation when the output is not the default one. 

## Example

### Python:
```python
import win_aec

mic = win_aec.AudioInputStream(None, None, 48000, 2, 32)
buf = bytearray(mic.buffer_size)
for _ in range(10):
    count, dropped = mic.read(buf)
    print(f"read {count} bytes, dropped_frames={dropped}")
```

### Rust:
```rust
use win_aec::AudioInputStream;

fn main() -> windows::core::Result<()> {
    let capture = AudioInputStream::new(None, None, 48_000, 2, 32)?;
    let mut buffer = vec![0u8; 4096];

    println!(
        "capture format: {} Hz, {} ch, {} bit",
        capture.sample_rate(),
        capture.channels(),
        capture.bits_per_sample()
    );

    for _ in 0..10 {
        let res = capture.read(&mut buffer, -1)?;
        println!("read {res.bytes_read} bytes");
    }

    Ok(())
}
```

## Notes

- This crate is Windows-only.
- `SetEchoCancellationRenderEndpoint` is attempted when the device exposes `IAcousticEchoCancellationControl`.
- On some machines or SDK versions, the interface is unavailable and the call falls back cleanly.
- `read(&mut buffer, timeout_ms)` drains all currently queued WASAPI packets into the caller buffer up to the buffer size. If the queue contains more audio than the caller buffer can hold, the function copies as much as fits and sets `dropped_frames` to `True`.
- `0` means non-blocking, negative values wait until the event is ready, and positive values wait for that many milliseconds.
- The library reads raw PCM packets directly from WASAPI and leaves decoding or DSP to the caller.
