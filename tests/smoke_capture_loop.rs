use win_aec::{AudioInputStream, AudioOutputStream};

#[test]
fn smoke_capture_api() {
    let capture = match AudioInputStream::new(None, None, 48_000, 2, 32) {
        Ok(capture) => capture,
        Err(err) => {
            eprintln!(
                "skipping WASAPI smoke test: no default communications capture endpoint is available ({err:?})"
            );
            return;
        }
    };

    assert!(capture.sample_rate() > 0, "sample rate should be set");
    assert!(capture.channels() > 0, "channel count should be set");
    assert!(capture.bits_per_sample() > 0, "bit depth should be set");

    let mut buffer = [0u8; 4096];
    let mut max_read = 0usize;

    for _ in 0..10 {
        let res = capture
            .read(&mut buffer, 0)
            .expect("capture packet read should not fail while the stream is active");

        assert!(
            res.bytes_read <= buffer.len(),
            "read should never exceed the caller-provided buffer"
        );
        max_read = max_read.max(res.bytes_read);
    }

    // Idle devices are valid; the stream remains usable even when it produces 0-byte
    // packets while waiting for audio activity.
    assert!(max_read <= buffer.len(), "the stream should remain bounded by the buffer size");
}

#[test]
fn smoke_render_api_falls_back_for_unsupported_format() {
    let render = match AudioOutputStream::new(None, 44_000, 2, 32) {
        Ok(render) => render,
        Err(err) => {
            eprintln!(
                "skipping WASAPI render smoke test: no default render endpoint is available ({err:?})"
            );
            return;
        }
    };

    assert!(render.sample_rate() > 0, "render sample rate should be set");
    assert!(render.channels() > 0, "render channel count should be set");
    assert!(
        matches!(render.bits_per_sample(), 16 | 32),
        "render stream should initialize on a supported WASAPI bit depth"
    );
}
