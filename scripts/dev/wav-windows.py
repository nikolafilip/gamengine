#!/usr/bin/env python3
"""Loudness of a 16-bit WAV in windows, for the sound gate (docs/SOUND.md 7).

    scripts/dev/wav-windows.py FILE.wav [--window MS] [--from S --to S]

Prints one line: `secs=... max_db=... min_db=... windows=N` for the RMS (dBFS, both channels
together) of every window of `--window` milliseconds (default 100) in the span, and
`--list` prints every window's dB as well. Silence is reported as -120 dB. Standard
library only: a gate runs where no numpy is.
"""

import argparse
import array
import math
import sys
import wave


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("file")
    ap.add_argument("--window", type=float, default=100.0, help="window length in ms")
    ap.add_argument("--from", dest="start", type=float, default=0.0)
    ap.add_argument("--to", dest="end", type=float, default=None)
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()
    with wave.open(args.file, "rb") as w:
        if w.getsampwidth() != 2:
            sys.exit(f"{args.file}: {w.getsampwidth() * 8}-bit samples; the gate reads 16-bit")
        rate, channels, frames = w.getframerate(), w.getnchannels(), w.getnframes()
        data = array.array("h")
        data.frombytes(w.readframes(frames))
    secs = frames / rate
    end = secs if args.end is None else min(args.end, secs)
    per_window = max(1, int(rate * args.window / 1000.0))
    first = int(args.start * rate)
    last = int(end * rate)
    dbs = []
    at = first
    while at + per_window <= last:
        chunk = data[at * channels : (at + per_window) * channels]
        if not chunk:
            break
        acc = 0.0
        for v in chunk:
            acc += (v / 32768.0) ** 2
        rms = math.sqrt(acc / len(chunk))
        dbs.append(20.0 * math.log10(rms) if rms > 0 else -120.0)
        if args.list:
            print(f"{at / rate:7.3f} s  {dbs[-1]:7.1f} dB")
        at += per_window
    if not dbs:
        sys.exit(f"{args.file}: no whole window between {args.start} and {end} s")
    print(f"secs={secs:.3f} max_db={max(dbs):.1f} min_db={min(dbs):.1f} windows={len(dbs)}")


if __name__ == "__main__":
    main()
