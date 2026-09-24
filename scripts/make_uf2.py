#!/usr/bin/env python3
"""Build a contiguous RP2040 UF2 image for dragging onto the RPI-RP2 mass storage device.

Usage:
    make_uf2.py OUT BASE OFFSET1 FILE1 [OFFSET2 FILE2 ...]
    make_uf2.py --selftest

`BASE` is the flash base address (e.g. 0x10000000). Each pair places `FILE` at
`BASE + OFFSET`. The tool emits ONE contiguous image from `BASE` to the highest
file end, filling any gap with 0x00.

Why contiguous: the RP2040 bootrom `virtual_disk.c` requires every UF2 block to
carry a 256-byte payload (short tail blocks are ignored) and tracks erased 4 KiB
sectors via `page_no = block_no * 256 / 4096`, which only holds when the blocks
are contiguous from the UF2 start address. A non-contiguous image (e.g. a
bootloader at 0x10000000 plus an app at 0x10006000 emitted as two runs) is
written to un-erased sectors and silently corrupted.
"""

import hashlib
import struct
import sys

MAGIC0 = 0x0A324655
MAGIC1 = 0x9E5D5157
MAGICE = 0x0AB16F30
FLAG_FAMILY = 0x00002000
RP2040_FAMILY = 0xE48BFF56
PAYLOAD = 256


def build(base, pairs):
    """Return (uf2_bytes, block_count, image_size) for the given placements."""
    end = 0
    for off, path in pairs:
        with open(path, "rb") as handle:
            end = max(end, off + len(handle.read()))
    end = (end + PAYLOAD - 1) // PAYLOAD * PAYLOAD
    image = bytearray(b"\x00" * end)
    for off, path in pairs:
        with open(path, "rb") as handle:
            data = handle.read()
        image[off:off + len(data)] = data

    total = end // PAYLOAD
    out = bytearray()
    for n in range(total):
        addr = base + n * PAYLOAD
        block = bytearray(512)
        struct.pack_into(
            "<IIIIIIII", block, 0, MAGIC0, MAGIC1, FLAG_FAMILY, addr, PAYLOAD, n, total,
            RP2040_FAMILY,
        )
        block[32:32 + PAYLOAD] = image[n * PAYLOAD:(n + 1) * PAYLOAD]
        struct.pack_into("<I", block, 508, MAGICE)
        out += block
    return bytes(out), total, end


def _selftest():
    import os
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        lower = os.path.join(tmp, "lower.bin")
        upper = os.path.join(tmp, "upper.bin")
        gap = 0x40
        with open(lower, "wb") as f:
            f.write(bytes(range(256)) * 2)  # 512 bytes at offset 0
        with open(upper, "wb") as f:
            f.write(bytes(reversed(range(256))))  # 256 bytes at the gap offset

        base = 0x10000000
        uf2, total, end = build(base, [(0, lower), (512 + gap, upper)])

        assert len(uf2) == total * 512
        image = bytearray()
        for n in range(total):
            block = uf2[n * 512:(n + 1) * 512]
            m0, m1, flags, addr, size, no, tot, family = struct.unpack("<IIIIIIII", block[:32])
            assert (m0, m1) == (MAGIC0, MAGIC1)
            assert struct.unpack("<I", block[508:512])[0] == MAGICE
            assert size == PAYLOAD, "every block must carry a 256-byte payload"
            assert no == n and tot == total, "blocks must be numbered linearly"
            assert addr == base + n * PAYLOAD, "blocks must be contiguous from base"
            image += block[32:32 + PAYLOAD]

        assert len(image) == end
        with open(lower, "rb") as f:
            expected_lower = f.read()
        with open(upper, "rb") as f:
            expected_upper = f.read()
        assert bytes(image[0:len(expected_lower)]) == expected_lower
        assert bytes(image[512 + gap:512 + gap + len(expected_upper)]) == expected_upper
        assert all(b == 0 for b in image[512:512 + gap]), "gap must be zero-filled"
    print("selftest ok")


def main(argv):
    if len(argv) == 2 and argv[1] == "--selftest":
        _selftest()
        return 0
    if len(argv) < 5 or (len(argv) - 3) % 2 != 0:
        print(__doc__)
        return 2

    out_path = argv[1]
    base = int(argv[2], 0)
    args = argv[3:]
    pairs = [(int(args[i], 0), args[i + 1]) for i in range(0, len(args), 2)]

    data, total, end = build(base, pairs)
    with open(out_path, "wb") as handle:
        handle.write(data)
    digest = hashlib.sha256(data).hexdigest()
    print(
        f"{out_path}: base={hex(base)} end={hex(base + end)} "
        f"{len(data)} bytes, {total} blocks, sha256={digest}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
