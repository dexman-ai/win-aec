use win_aec::AudioInputStream;

fn main() -> windows::core::Result<()> {
    let capture = AudioInputStream::new(None, None, 48_000, 2, 32)?;
    let mut my_buf = vec![0u8; 4096];

    println!(
        "capture format: {} Hz, {} ch, {} bit",
        capture.sample_rate(),
        capture.channels(),
        capture.bits_per_sample()
    );
    println!(
        "echo cancellation endpoint explicitly bound: {}",
        capture.echo_cancellation_endpoint_bound()
    );

    for packet_index in 0..10 {
        let bytes_read = capture.read(&mut my_buf, 0)?;
        if bytes_read == 0 {
            println!("packet {packet_index}: no audio data yet");
            continue;
        }

        println!("packet {packet_index}: bytes={bytes_read}");
    }

    Ok(())
    // Dropping the stream stops the WASAPI pipeline and tears down COM.
}
