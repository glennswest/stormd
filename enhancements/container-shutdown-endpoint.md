# Enhancement: Container Shutdown REST Endpoint

## Summary

Add a REST API endpoint that causes stormd (PID 1) to gracefully shut down the entire container. This enables supervised processes to trigger a container restart when they detect a newer image version is available on the registry.

## Motivation

mkube-agent runs inside a stormd-supervised container on bare metal hosts. When a new agent image is pushed to the registry, the agent process needs the **container** to restart (not just the process) so the fresh image is pulled. Currently stormd only restarts the process within the same container, reusing the old binary.

## API

```
POST /api/v1/shutdown
```

**Response:** `200 OK` with body `{"status": "shutting down"}`

**Behavior:**
1. Stops all supervised processes gracefully (SIGTERM, then SIGKILL after timeout)
2. stormd exits with code 0
3. Container exits (PID 1 died)
4. External restart mechanism (systemd, podman --restart=always) recreates the container with a fresh image pull

## Optional: Exit code parameter

```json
POST /api/v1/shutdown
{"exitCode": 42}
```

Allow the caller to specify the exit code stormd uses. This lets the restart mechanism distinguish between "please update me" (code 42) vs normal shutdown (code 0) vs crash (non-zero).

## Consumer

- `mkube-agent` — checks registry for new image digest between jobs. On mismatch, calls `POST http://localhost:9080/api/v1/shutdown` to trigger container-level restart.
