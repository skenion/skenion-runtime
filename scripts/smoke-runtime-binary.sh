#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <runtime-binary>" >&2
  exit 2
fi

runtime_bin="$1"

if [[ ! -f "${runtime_bin}" ]]; then
  echo "runtime binary not found: ${runtime_bin}" >&2
  exit 1
fi

"${runtime_bin}" --help >/dev/null
"${runtime_bin}" serve --help >/dev/null
"${runtime_bin}" validate-project --project fixtures/current-0.1/valid/clear-color-render.project.json
"${runtime_bin}" plan --project fixtures/current-0.1/valid/clear-color-render.project.json --format json >/dev/null
"${runtime_bin}" run --project fixtures/current-0.1/valid/clear-color-render.project.json --frames 2 --format json >/dev/null

port="$((37610 + ($$ % 1000)))"
server_log="$(mktemp)"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  rm -f "${server_log}"
}
trap cleanup EXIT

SKENION_PREVIEW_DRY_RUN=1 "${runtime_bin}" serve --host 127.0.0.1 --port "${port}" >"${server_log}" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 20); do
  if curl --fail --silent "http://127.0.0.1:${port}/health" >/dev/null; then
    exit 0
  fi

  if ! kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
    echo "runtime server exited before health check passed" >&2
    cat "${server_log}" >&2
    exit 1
  fi

  sleep 1
done

echo "runtime health endpoint did not become ready" >&2
cat "${server_log}" >&2
exit 1
