"""Retry policy shared by the four OpenAI-compatible experiment clients."""

from urllib.parse import urlsplit


def is_retryable_ollama_xml_500(url: str, status: int, body: str) -> bool:
    host = urlsplit(url).hostname
    return (
        host in {"localhost", "127.0.0.1", "::1"}
        and status == 500
        and "xml syntax error" in body.lower()
    )
