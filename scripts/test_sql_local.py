#!/usr/bin/env python3
"""Disposable loopback PostgreSQL/MySQL tests, without Docker or system services."""
import argparse
import contextlib
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = [
    "postgresql-16", "postgresql-client-16",
    "mysql-server-core-8.0", "mysql-client-core-8.0",
    "libevent-pthreads-2.1-7t64", "libprotobuf-lite32t64", "libaio1t64",
]


def run(args, *, env=None, **kwargs):
    return subprocess.run(args, env=env, check=True, **kwargs)


def bootstrap(destination):
    """Download apt-signed repository packages; never install into the system."""
    if "Ubuntu 24.04" not in Path("/etc/os-release").read_text():
        raise RuntimeError("--bootstrap-ubuntu supports Ubuntu 24.04 only")
    # Refuse to overwrite/adopt an existing tree.
    destination.mkdir(parents=True, exist_ok=False)
    run(["apt-get", "download", *PACKAGES], cwd=destination)
    root = destination / "root"
    for package in sorted(destination.glob("*.deb")):
        run(["dpkg-deb", "-x", str(package), str(root)])
    return root


def free_port():
    # The close/bind race fails safely at server startup; no existing server is used.
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@contextlib.contextmanager
def server(args, log_path, env):
    with log_path.open("wb") as log:
        process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
        try:
            yield process
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)


def ready(process, probe, env, log):
    def diagnostic():
        text = log.read_text()[-12000:]
        for name in ("PGPASSWORD", "MYSQL_PWD"):
            if env.get(name):
                text = text.replace(env[name], "[redacted]")
        return text
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Test server exited; log:\n{diagnostic()}")
        result = subprocess.run(probe, env=env, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=10)
        # Even if a foreign server occupies the port, require our process alive.
        if result.returncode == 0 and process.poll() is None:
            return
        time.sleep(0.5)
    raise RuntimeError(f"Readiness timed out; log:\n{diagnostic()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--tools-root", type=Path,
                       help="unpacked Ubuntu package root containing usr/")
    group.add_argument("--bootstrap-ubuntu", type=Path,
                       help="download/extract packages to a NEW directory (no sudo)")
    parser.add_argument("--cargo-test-args", nargs=argparse.REMAINDER, default=[])
    args = parser.parse_args()
    tools = (bootstrap(args.bootstrap_ubuntu.resolve()) if args.bootstrap_ubuntu
             else args.tools_root.resolve())
    pg = tools / "usr/lib/postgresql/16/bin"
    mysql = tools / "usr/bin/mysql"
    mysqld = tools / "usr/sbin/mysqld"
    for executable in (pg / "postgres", pg / "initdb", pg / "psql", mysql, mysqld):
        if not executable.is_file():
            raise RuntimeError(f"Missing test executable: {executable}")
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = str(tools / "usr/lib/x86_64-linux-gnu") + (
        ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    password = secrets.token_hex(24)
    env.update(PGPASSWORD=password, MYSQL_PWD=password)
    parent = Path("/tmp/opencode")
    parent.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="bbgo-native-sql-", dir=parent) as directory:
        directory = Path(directory)
        directory.chmod(0o700)
        pg_data, mysql_data = directory / "pg", directory / "mysql"
        pwfile = directory / "pg-password"
        pwfile.write_text(password)
        pwfile.chmod(0o600)
        run([str(pg / "initdb"), "-D", str(pg_data), "-L",
             str(tools / "usr/share/postgresql/16"), "-U", "bbgo",
             "--auth-local=trust", "--auth-host=scram-sha-256",
             "--pwfile", str(pwfile), "--no-locale"], env=env,
            stdout=subprocess.DEVNULL)
        # --no-defaults prevents adopting machine/user MySQL configuration.
        mysql_common = [str(mysqld), "--no-defaults", f"--basedir={tools / 'usr'}",
                        f"--datadir={mysql_data}",
                        f"--lc-messages-dir={tools / 'usr/share/mysql'}"]
        with (directory / "mysql-init.log").open("wb") as init_log:
            run([*mysql_common, "--initialize-insecure"], env=env,
                stdout=subprocess.DEVNULL, stderr=init_log)
        initfile = directory / "mysql-init.sql"
        initfile.write_text(
            f"ALTER USER 'root'@'localhost' IDENTIFIED BY '{password}';\n"
            "CREATE DATABASE bbgo_backtest_test;\n")
        initfile.chmod(0o600)
        pg_port, mysql_port = free_port(), free_port()
        while mysql_port == pg_port:
            mysql_port = free_port()
        pg_command = [str(pg / "postgres"), "-D", str(pg_data),
                      "-h", "127.0.0.1", "-p", str(pg_port), "-k", str(directory)]
        mysql_command = [*mysql_common, "--bind-address=127.0.0.1",
                         f"--port={mysql_port}", "--mysqlx=OFF",
                         "--secure-file-priv=NULL",
                         f"--socket={directory / 'mysql.sock'}",
                         f"--pid-file={directory / 'mysql.pid'}",
                         f"--init-file={initfile}"]
        pg_log, mysql_log = directory / "postgres.log", directory / "mysql.log"
        with server(pg_command, pg_log, env) as postgres, \
                server(mysql_command, mysql_log, env) as my:
            pg_probe = [str(pg / "psql"), "-h", "127.0.0.1", "-p", str(pg_port),
                        "-U", "bbgo", "-d", "postgres", "-v", "ON_ERROR_STOP=1"]
            ready(postgres, [*pg_probe, "-c", "SELECT 1"], env, pg_log)
            run([*pg_probe, "-c", "CREATE DATABASE bbgo_backtest_test"], env=env,
                stdout=subprocess.DEVNULL)
            my_probe = [str(mysql), "--no-defaults", "-h", "127.0.0.1",
                        "-P", str(mysql_port), "-u", "root",
                        "-D", "bbgo_backtest_test", "-e", "SELECT 1"]
            ready(my, my_probe, env, mysql_log)
            env.update(
                BBGO_ALLOW_TEST_DATABASE_WRITES="1",
                BBGO_TEST_POSTGRES_URL=f"postgres://bbgo:{password}@127.0.0.1:{pg_port}/bbgo_backtest_test",
                BBGO_TEST_MYSQL_URL=f"mysql://root:{password}@127.0.0.1:{mysql_port}/bbgo_backtest_test",
            )
            print("Running native SQL tests on fresh loopback-only PostgreSQL/MySQL.", flush=True)
            run(["cargo", "test", "-p", "bbgo-backtest", "-p", "bbgo-persistence", "--test", "server_sql",
                 "--locked", *args.cargo_test_args, "--", "--ignored"], env=env, cwd=ROOT)


if __name__ == "__main__":
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from None
