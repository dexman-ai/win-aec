use win_aec::AudioInputStream;

fn main() -> windows::core::Result<()> {
    let capture = AudioInputStream::new(None, None, 48_000, 2, 16)?;
    let mut my_buf = vec![0u8; 4096];

    println!(
        "capture format: {} Hz, {} ch, {} bit",
        capture.sample_rate(),
        capture.channels(),
        capture.bits_per_sample()
    );
    println!(
        "capture buffer size: {} bytes",
        capture.buffer_size()
    );
    println!(
        "echo cancellation endpoint explicitly bound: {}",
        capture.echo_cancellation_endpoint_bound()
    );

    for packet_index in 0..10 {
        let res = capture.read(&mut my_buf, -1)?;
        println!("packet {packet_index}: bytes={}, dropped_frames={}", res.bytes_read, res.dropped_frames);
    }

    Ok(())
    // Dropping the stream stops the WASAPI pipeline and tears down COM.
}
