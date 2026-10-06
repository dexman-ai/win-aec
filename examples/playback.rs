use std::time::Duration;
use win_aec::AudioOutputStream;

fn main() -> windows::core::Result<()> {
    let render = AudioOutputStream::new(None, 48_000, 2, 16)?;

    println!(
        "render format: {} Hz, {} ch, {} bit",
        render.sample_rate(),
        render.channels(),
        render.bits_per_sample()
    );

    let samples_per_frame = render.channels() as usize;
    let sample_bytes = render.bits_per_sample() as usize / 8;
    let frame_bytes = samples_per_frame * sample_bytes;

    for i in 0..10 {
        let frame_count = 480; // 10 ms at 48 kHz
        let mut data = Vec::with_capacity(frame_count * frame_bytes);

        for frame in 0..frame_count {
            let t = (i * 480 + frame) as f64;
            let left = ((t * 0.1).sin() * 12000.0) as i16;
            let right = ((t * 0.13).sin() * 12000.0) as i16;
            data.extend_from_slice(&left.to_le_bytes());
            data.extend_from_slice(&right.to_le_bytes());
        }

        let written = render.write(&data, -1)?;
        println!("packet {i}: wrote {written} bytes");
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}
