# capture example in Python
import win_aec

# The requested format is only a best-effort hint. WASAPI may negotiate a
# different sample rate, channel count, or dtype for the device.
mic = win_aec.AudioInputStream(
    mic_id=None,
    reference_output_id=None,
    sample_rate=24_000,
    channels=1,
    dtype="int16",
)

print(f"actual format: {mic.sample_rate} Hz, {mic.channels} ch, {mic.dtype}")
print(f"buffer size: {mic.buffer_size} bytes")
print(f"echo cancellation endpoint bound: {mic.echo_cancellation_endpoint_bound}")

# Use the negotiated format for any downstream conversion.
buffer = bytearray(mic.buffer_size)

for i in range(10):
    frame_count, dropped_frames = mic.read(buffer, timeout=1.0)
    print(f"read {frame_count} frames, dropped_frames={dropped_frames}")


