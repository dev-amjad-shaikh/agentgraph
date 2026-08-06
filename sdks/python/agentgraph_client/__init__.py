"""agentgraph-client: zero-dependency Python SDK for agentgraph-server.

A stdlib-only (``urllib.request`` + ``json``) HTTP/SSE client for the
``agentgraph-server`` HTTP API: threads, runs (background / blocking /
SSE-streaming), checkpoint history, time travel (fork + replay),
assistants, crons, and the cross-thread KV store.

Quickstart::

    from agentgraph_client import AgentGraphClient

    client = AgentGraphClient("http://127.0.0.1:8100")
    print(client.info())

    thread = client.create_thread("pipeline")
    result = client.run_wait(thread["thread_id"])
    print(result["status"], result["output"])

No third-party packages are required — Python 3.8+ is enough.
"""

from .client import AgentGraphClient, AgentGraphError, SSEEvent

__all__ = ["AgentGraphClient", "AgentGraphError", "SSEEvent"]
__version__ = "0.1.0"
