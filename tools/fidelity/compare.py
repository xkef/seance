#!/usr/bin/env python3
import argparse
import json
import math
import os
import struct
import subprocess
import tempfile
from pathlib import Path


def convert_to_bmp(src: Path, dst: Path) -> None:
    subprocess.run(
        ["sips", "-s", "format", "bmp", str(src), "--out", str(dst)],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def channel_mask_info(mask: int) -> tuple[int, int]:
    if mask == 0:
        raise ValueError("BMP channel mask must be non-zero")
    shift = (mask & -mask).bit_length() - 1
    bits = (mask >> shift).bit_length()
    return shift, bits


def extract_channel(value: int, mask: int) -> int:
    shift, bits = channel_mask_info(mask)
    raw = (value & mask) >> shift
    if bits == 8:
        return raw
    return round(raw * 255 / ((1 << bits) - 1))


def read_bmp(path: Path) -> tuple[int, int, bytearray]:
    data = path.read_bytes()
    if data[:2] != b"BM":
        raise ValueError(f"{path} is not a BMP file")

    pixel_offset = struct.unpack_from("<I", data, 10)[0]
    dib_size = struct.unpack_from("<I", data, 14)[0]
    if dib_size < 40:
        raise ValueError(f"{path} has unsupported DIB header size {dib_size}")

    width = struct.unpack_from("<i", data, 18)[0]
    height = struct.unpack_from("<i", data, 22)[0]
    planes = struct.unpack_from("<H", data, 26)[0]
    bits_per_pixel = struct.unpack_from("<H", data, 28)[0]
    compression = struct.unpack_from("<I", data, 30)[0]

    if planes != 1:
        raise ValueError(f"{path} has unsupported plane count {planes}")
    if bits_per_pixel not in (24, 32):
        raise ValueError(f"{path} uses unsupported pixel format {bits_per_pixel}-bit")
    if compression not in (0, 3):
        raise ValueError(f"{path} uses unsupported compression mode {compression}")

    top_down = height < 0
    width = abs(width)
    height = abs(height)
    bytes_per_pixel = bits_per_pixel // 8
    row_stride = ((width * bits_per_pixel + 31) // 32) * 4

    masks = None
    if compression == 3:
        if bits_per_pixel != 32:
            raise ValueError(f"{path} uses unsupported bitfields format {bits_per_pixel}-bit")
        masks = struct.unpack_from("<IIII", data, 54)

    pixels = bytearray(width * height * 3)
    for row in range(height):
        src_row = row if top_down else height - 1 - row
        src_base = pixel_offset + src_row * row_stride
        dst_base = row * width * 3
        for col in range(width):
            src = src_base + col * bytes_per_pixel
            dst = dst_base + col * 3
            if masks is None:
                b = data[src]
                g = data[src + 1]
                r = data[src + 2]
            else:
                packed = struct.unpack_from("<I", data, src)[0]
                red_mask, green_mask, blue_mask, _alpha_mask = masks
                r = extract_channel(packed, red_mask)
                g = extract_channel(packed, green_mask)
                b = extract_channel(packed, blue_mask)
            pixels[dst] = r
            pixels[dst + 1] = g
            pixels[dst + 2] = b
    return width, height, pixels


def write_bmp(path: Path, width: int, height: int, pixels: bytearray) -> None:
    row_stride = ((width * 3 + 3) // 4) * 4
    image_size = row_stride * height
    file_size = 14 + 40 + image_size

    with path.open("wb") as fh:
        fh.write(b"BM")
        fh.write(struct.pack("<IHHI", file_size, 0, 0, 54))
        fh.write(struct.pack("<IIIHHIIIIII", 40, width, height, 1, 24, 0, image_size, 2835, 2835, 0, 0))

        padding = b"\x00" * (row_stride - width * 3)
        for row in range(height - 1, -1, -1):
            row_base = row * width * 3
            for col in range(width):
                src = row_base + col * 3
                r = pixels[src]
                g = pixels[src + 1]
                b = pixels[src + 2]
                fh.write(bytes((b, g, r)))
            fh.write(padding)


def write_png_via_sips(path: Path, width: int, height: int, pixels: bytearray) -> None:
    with tempfile.TemporaryDirectory(prefix="seance-fidelity-bmp-") as tmp:
        bmp_path = Path(tmp) / "diff.bmp"
        write_bmp(bmp_path, width, height, pixels)
        subprocess.run(
            ["sips", "-s", "format", "png", str(bmp_path), "--out", str(path)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )


def crop_top_left(pixels: bytearray, width: int, height: int, target_width: int, target_height: int) -> bytearray:
    if (width, height) == (target_width, target_height):
        return pixels
    cropped = bytearray(target_width * target_height * 3)
    for row in range(target_height):
        src_base = row * width * 3
        dst_base = row * target_width * 3
        span = target_width * 3
        cropped[dst_base : dst_base + span] = pixels[src_base : src_base + span]
    return cropped


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare two screenshots and emit a diff image plus JSON metrics.")
    parser.add_argument("--reference", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--diff", required=True, type=Path)
    parser.add_argument("--metrics", required=True, type=Path)
    parser.add_argument("--scale", type=int, default=4, help="Diff amplification factor for the output image")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="seance-fidelity-") as tmp:
        tmpdir = Path(tmp)
        ref_bmp = tmpdir / "reference.bmp"
        cand_bmp = tmpdir / "candidate.bmp"
        convert_to_bmp(args.reference, ref_bmp)
        convert_to_bmp(args.candidate, cand_bmp)
        width, height, reference = read_bmp(ref_bmp)
        cand_width, cand_height, candidate = read_bmp(cand_bmp)

    original_width, original_height = width, height
    original_cand_width, original_cand_height = cand_width, cand_height
    crop_width = min(width, cand_width)
    crop_height = min(height, cand_height)
    reference = crop_top_left(reference, width, height, crop_width, crop_height)
    candidate = crop_top_left(candidate, cand_width, cand_height, crop_width, crop_height)
    width, height = crop_width, crop_height

    total_channels = width * height * 3
    total_pixels = width * height
    diff_pixels = bytearray(total_channels)
    tile_size = 32
    tile_stats: dict[tuple[int, int], dict[str, int]] = {}

    sum_abs = 0
    sum_sq = 0
    differing_pixels = 0
    max_diff = -1
    worst = {"x": 0, "y": 0, "delta": [0, 0, 0], "max_channel_diff": 0}

    for pixel_index in range(total_pixels):
        base = pixel_index * 3
        dr = abs(reference[base] - candidate[base])
        dg = abs(reference[base + 1] - candidate[base + 1])
        db = abs(reference[base + 2] - candidate[base + 2])
        pixel_max = max(dr, dg, db)

        diff_pixels[base] = min(dr * args.scale, 255)
        diff_pixels[base + 1] = min(dg * args.scale, 255)
        diff_pixels[base + 2] = min(db * args.scale, 255)

        sum_abs += dr + dg + db
        sum_sq += dr * dr + dg * dg + db * db
        if pixel_max > 0:
            differing_pixels += 1

        if pixel_max > max_diff:
            x = pixel_index % width
            y = pixel_index // width
            max_diff = pixel_max
            worst = {
                "x": x,
                "y": y,
                "delta": [dr, dg, db],
                "max_channel_diff": pixel_max,
            }

        tile_x = (pixel_index % width) // tile_size
        tile_y = (pixel_index // width) // tile_size
        tile = tile_stats.setdefault((tile_x, tile_y), {"sum": 0, "max": 0, "count": 0})
        tile["sum"] += pixel_max
        tile["count"] += 1
        if pixel_max > tile["max"]:
            tile["max"] = pixel_max

    hot_tiles = []
    for (tile_x, tile_y), tile in tile_stats.items():
        avg = tile["sum"] / tile["count"] if tile["count"] else 0.0
        hot_tiles.append(
            {
                "x": tile_x * tile_size,
                "y": tile_y * tile_size,
                "width": min(tile_size, width - tile_x * tile_size),
                "height": min(tile_size, height - tile_y * tile_size),
                "avg_diff": round(avg, 4),
                "max_diff": tile["max"],
            }
        )
    hot_tiles.sort(key=lambda tile: (tile["avg_diff"], tile["max_diff"]), reverse=True)

    metrics = {
        "reference": os.fspath(args.reference),
        "candidate": os.fspath(args.candidate),
        "diff": os.fspath(args.diff),
        "width": width,
        "height": height,
        "reference_dimensions": {"width": original_width, "height": original_height},
        "candidate_dimensions": {"width": original_cand_width, "height": original_cand_height},
        "cropped_to_common_top_left": (original_width, original_height) != (original_cand_width, original_cand_height),
        "mae": round(sum_abs / total_channels, 6),
        "rmse": round(math.sqrt(sum_sq / total_channels), 6),
        "max_channel_diff": max_diff,
        "differing_pixels": differing_pixels,
        "differing_ratio": round(differing_pixels / total_pixels, 6),
        "worst_pixel": worst,
        "hot_tiles": hot_tiles[:8],
    }

    args.metrics.write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf-8")
    write_png_via_sips(args.diff, width, height, diff_pixels)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
