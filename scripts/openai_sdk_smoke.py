#!/usr/bin/env python3
"""Optional smoke checks using the official OpenAI Python SDK.

Install `openai` in a local virtualenv before running:

    python -m pip install openai
    RUSTYGATE_GATEWAY_API_KEY=... python scripts/openai_sdk_smoke.py
"""

import os
import sys


def main() -> int:
    try:
        from openai import OpenAI
    except ImportError:
        print("SKIP install the official `openai` package to run SDK smoke checks")
        return 0

    api_key = os.environ.get("RUSTYGATE_GATEWAY_API_KEY")
    if not api_key:
        print("FAIL RUSTYGATE_GATEWAY_API_KEY must be set")
        return 1

    base_url = os.environ.get("BASE_URL", "http://127.0.0.1:8080")
    client = OpenAI(api_key=api_key, base_url=f"{base_url.rstrip('/')}/v1")

    models = client.models.list()
    if not models.data:
        print("FAIL expected at least one model")
        return 1

    response = client.responses.create(
        model=models.data[0].id,
        input="Smoke check: answer with one short sentence.",
    )
    if response.object != "response":
        print(f"FAIL unexpected response object: {response.object}")
        return 1

    embedding = client.embeddings.create(
        model="text-embedding-3-small",
        input="Smoke check",
    )
    if not embedding.data or not embedding.data[0].embedding:
        print("FAIL expected embedding data")
        return 1

    print("PASS official OpenAI SDK smoke checks completed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
