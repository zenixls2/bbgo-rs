#!/usr/bin/env bash
# Only this invocation's uniquely named stack and volumes are ever removed.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
command -v docker >/dev/null || { echo "Docker is required; install Docker Engine first." >&2; exit 1; }
command -v python3 >/dev/null || { echo "Python 3 is required for random credentials." >&2; exit 1; }
if [[ "$(docker info --format '{{.Swarm.ControlAvailable}}')" != true ]]; then
    echo "A local Swarm manager is required. Run cargo sql-swarm-init first." >&2
    exit 1
fi
# Remote Docker contexts would expose credentials and connect tests to the wrong host.
endpoint="$(docker context inspect --format '{{.Endpoints.docker.Host}}')"
if [[ -n "${DOCKER_HOST:-}" ]]; then endpoint="$DOCKER_HOST"; fi
[[ "$endpoint" == unix://* ]] || { echo "Only a local Unix-socket Docker daemon is supported." >&2; exit 1; }
export BBGO_SWARM_NODE
BBGO_SWARM_NODE="$(docker info --format '{{.Swarm.NodeID}}')"
export BBGO_SWARM_PASSWORD
BBGO_SWARM_PASSWORD="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
export BBGO_SWARM_MYSQL_PORT="${BBGO_SWARM_MYSQL_PORT:-13306}"
export BBGO_SWARM_POSTGRES_PORT="${BBGO_SWARM_POSTGRES_PORT:-15432}"
for port in "$BBGO_SWARM_MYSQL_PORT" "$BBGO_SWARM_POSTGRES_PORT"; do
    [[ "$port" =~ ^[0-9]{1,5}$ ]] && (( 10#$port >= 1024 && 10#$port <= 65535 )) || {
        echo "Test ports must be integers between 1024 and 65535." >&2; exit 1;
    }
done
[[ "$BBGO_SWARM_MYSQL_PORT" != "$BBGO_SWARM_POSTGRES_PORT" ]] || { echo "Test ports must differ." >&2; exit 1; }
stack="bbgo-sql-$(python3 -c 'import secrets; print(secrets.token_hex(8))')"
cleanup() {
    local result=$?
    trap - EXIT
    if ! docker stack rm "$stack" >/dev/null; then
        echo "Cleanup failed: manually remove stack $stack and its volumes." >&2
        exit 1
    fi
    # Stack removal is asynchronous. Never force-remove a running container/volume.
    for ((i=0; i<60; i++)); do
        if [[ -z "$(docker ps -aq --filter "label=com.docker.stack.namespace=$stack")" ]]; then
            local volume
            for volume in "${stack}_mysql-data" "${stack}_postgres-data"; do
                if docker volume inspect "$volume" >/dev/null 2>&1; then
                    docker volume rm "$volume" >/dev/null || result=1
                fi
            done
            exit "$result"
        fi
        sleep 1
    done
    echo "Cleanup timed out; remove only stack $stack and its named volumes after tasks stop." >&2
    exit 1
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "Starting disposable stack $stack (ports $BBGO_SWARM_MYSQL_PORT/$BBGO_SWARM_POSTGRES_PORT)."
echo "Swarm publishes on all host interfaces; use only a trusted/firewalled development host."
docker stack deploy --detach=true -c scripts/sql-swarm.yml "$stack" >/dev/null
for service in mysql postgres; do
    ready=false
    for ((i=0; i<180; i++)); do
        container="$(docker ps -q --filter "label=com.docker.swarm.service.name=${stack}_${service}" --filter status=running)"
        if [[ -n "$container" ]] && [[ "$(docker inspect --format '{{.State.Health.Status}}' "$container")" == healthy ]]; then
            ready=true
            break
        fi
        sleep 2
    done
    if [[ "$ready" != true ]]; then
        docker service ps --no-trunc "${stack}_${service}" >&2
        echo "$service did not become healthy; tests were not run." >&2
        exit 1
    fi
done
export BBGO_ALLOW_TEST_DATABASE_WRITES=1
export BBGO_TEST_MYSQL_URL="mysql://root:${BBGO_SWARM_PASSWORD}@127.0.0.1:${BBGO_SWARM_MYSQL_PORT}/bbgo_backtest_test"
export BBGO_TEST_POSTGRES_URL="postgres://postgres:${BBGO_SWARM_PASSWORD}@127.0.0.1:${BBGO_SWARM_POSTGRES_PORT}/bbgo_backtest_test"
cargo test -p bbgo-backtest -p bbgo-persistence --test server_sql --locked "$@" -- --ignored
