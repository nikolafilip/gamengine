#!/usr/bin/env python3
"""Ask Gemini (Google AI Studio) for a design or code review. Development helper only;
CI never runs it and nothing in the build depends on it.

    scripts/dev/gemini-review.py --prompt "Review this protocol for ..." \
        --file docs/PROTOCOL.md --file crates/gm-net/src/bits.rs --out /tmp/review.md

    --prompt-file FILE   read the prompt from a file instead of --prompt
    --image FILE         attach a PNG/JPEG (repeatable): a screenshot to look at
    --model NAME         default gemini-3.1-pro-preview (gemini-3.8-flash is faster; add --search
                         for Google-grounded answers, which only the flash models accept)
    --temp T             sampling temperature, default 0.3
    --max-tokens N       default 16384

API key: $GEMINI_API_KEY, else ~/google-ai-studio-api-key. The files are sent verbatim, so do
not pass secrets. The answer is advice to weigh, not instructions to follow.
"""

import argparse
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

API = "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"


def api_key() -> str:
    key = os.environ.get("GEMINI_API_KEY")
    if key:
        return key.strip()
    path = Path.home() / "google-ai-studio-api-key"
    if path.is_file():
        return path.read_text().strip()
    sys.exit("no API key: set GEMINI_API_KEY or create ~/google-ai-studio-api-key")


def call(model: str, parts: list, temp: float, max_tokens: int, search: bool, tries: int = 3) -> str:
    body = {
        "contents": [{"role": "user", "parts": parts}],
        "generationConfig": {"temperature": temp, "maxOutputTokens": max_tokens},
    }
    if search:
        body["tools"] = [{"google_search": {}}]
    data = json.dumps(body).encode()
    req = urllib.request.Request(
        API.format(model=model),
        data=data,
        headers={"x-goog-api-key": api_key(), "Content-Type": "application/json"},
        method="POST",
    )
    for attempt in range(tries):
        try:
            with urllib.request.urlopen(req, timeout=900) as resp:
                doc = json.load(resp)
            cand = doc["candidates"][0]
            text = "".join(p.get("text", "") for p in cand["content"]["parts"])
            meta = cand.get("groundingMetadata", {})
            sources = [
                f'{c.get("web", {}).get("uri", "")} | {c.get("web", {}).get("title", "")}'
                for c in meta.get("groundingChunks", [])
            ]
            if sources:
                text += "\n\n### Grounding sources\n" + "\n".join(sources)
            usage = doc.get("usageMetadata", {})
            print(
                f"{model}: {usage.get('promptTokenCount', '?')} prompt tokens, "
                f"{usage.get('candidatesTokenCount', '?')} answer tokens, "
                f"finish={cand.get('finishReason', '?')}",
                file=sys.stderr,
            )
            return text
        except urllib.error.HTTPError as e:
            print(f"{model}: HTTP {e.code}: {e.read()[:400]!r}", file=sys.stderr)
        except Exception as e:  # noqa: BLE001 - report and retry
            print(f"{model}: {e}", file=sys.stderr)
        time.sleep(10 * (attempt + 1))
    sys.exit("gemini request failed")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--prompt")
    ap.add_argument("--prompt-file")
    ap.add_argument("--file", action="append", default=[], help="attach a text file (repeatable)")
    ap.add_argument("--image", action="append", default=[], help="attach a PNG/JPEG (repeatable)")
    ap.add_argument("--model", default="gemini-3.1-pro-preview")
    ap.add_argument("--temp", type=float, default=0.3)
    ap.add_argument("--max-tokens", type=int, default=16384)
    ap.add_argument("--search", action="store_true", help="enable Google Search grounding")
    ap.add_argument("--out", help="write the answer here (also printed)")
    args = ap.parse_args()

    if args.prompt_file:
        prompt = Path(args.prompt_file).read_text()
    elif args.prompt:
        prompt = args.prompt
    else:
        sys.exit("need --prompt or --prompt-file")

    parts = [{"text": prompt}]
    for f in args.file:
        p = Path(f)
        parts.append({"text": f"\n\n===== FILE: {f} =====\n{p.read_text()}\n===== END FILE: {f} =====\n"})

    for f in args.image:
        p = Path(f)
        mime = "image/jpeg" if p.suffix.lower() in (".jpg", ".jpeg") else "image/png"
        parts.append({"text": f"\n\n===== IMAGE: {f} =====\n"})
        parts.append({"inline_data": {"mime_type": mime, "data": base64.b64encode(p.read_bytes()).decode()}})

    answer = call(args.model, parts, args.temp, args.max_tokens, args.search)
    if args.out:
        Path(args.out).write_text(answer)
        print(f"wrote {args.out} ({len(answer)} chars)", file=sys.stderr)
    print(answer)


if __name__ == "__main__":
    main()
