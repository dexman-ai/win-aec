# playback example in Python
import math
import time
import struct

import win_aec

render = win_aec.AudioOutputStream(
    device_id=None,
    sample_rate=48_000,
    channels=2,
    dtype="int16",
)

print(f"render format: {render.sample_rate} Hz, {render.channels} ch, {render.dtype}")

samples_per_frame = render.channels
sample_bytes = 2  # int16
frame_bytes = samples_per_frame * sample_bytes

for i in range(10):
    frame_count = 480  # 10 ms at 48 kHz
    data = bytearray()

    for frame in range(frame_count):
        t = float(i * 480 + frame)
        left = int(math.sin(t * 0.1) * 12000.0)
        right = int(math.sin(t * 0.13) * 12000.0)
        data.extend(struct.pack("<hh", left, right))

    written = render.write(data, timeout=1.0)
    print(f"packet {i}: wrote {written} samples")
    time.sleep(0.01)
