"""Offline lifecycle checks; Docker and Cargo are mocked, not database tests."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
MOCK = r'''
import json, os, pathlib, sys
tool = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["MOCK_LOG"], "a") as log:
    log.write(json.dumps([tool, *args]) + "\n")
if tool == "cargo":
    assert os.environ["BBGO_ALLOW_TEST_DATABASE_WRITES"] == "1"
    for name in ("MYSQL", "POSTGRES"):
        assert os.environ[f"BBGO_TEST_{name}_URL"].endswith("/bbgo_backtest_test")
    sys.exit(int(os.environ.get("MOCK_TEST_EXIT", "0")))
if args[:1] == ["info"]:
    print("true" if "ControlAvailable" in args[-1] else "local-node")
elif args[:2] == ["context", "inspect"]:
    print(os.environ.get("MOCK_ENDPOINT", "unix:///var/run/docker.sock"))
elif args[:2] == ["stack", "deploy"]:
    sys.exit(int(os.environ.get("MOCK_DEPLOY_EXIT", "0")))
elif args[:2] == ["ps", "-q"]:
    print("test-container")
elif args[:1] == ["inspect"]:
    print("healthy")
'''


class SwarmScript(unittest.TestCase):
    def invoke(self, **overrides):
        # Only owned temporary files are removed by TemporaryDirectory.
        with tempfile.TemporaryDirectory(prefix="bbgo-swarm-script-",
                                         dir="/tmp/opencode") as directory:
            directory = Path(directory)
            for name in ("docker", "cargo"):
                executable = directory / name
                executable.write_text(f"#!{sys.executable}\n{MOCK}")
                executable.chmod(0o700)
            env = dict(os.environ)
            for name in ("DOCKER_HOST", "BBGO_SWARM_MYSQL_PORT",
                         "BBGO_SWARM_POSTGRES_PORT"):
                env.pop(name, None)
            env.update(PATH=f"{directory}:{env['PATH']}",
                       MOCK_LOG=str(directory / "calls"), **overrides)
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/test-sql-swarm.sh")],
                env=env, capture_output=True, text=True, timeout=10,
            )
            calls = [json.loads(line) for line in
                     (directory / "calls").read_text().splitlines()]
            return result, calls

    def assert_cleanup(self, calls):
        deploy = next(call for call in calls if call[1:3] == ["stack", "deploy"])
        stack = deploy[-1]
        self.assertTrue(stack.startswith("bbgo-sql-"))
        self.assertIn(["docker", "stack", "rm", stack], calls)
        removals = [call[-1] for call in calls if call[1:3] == ["volume", "rm"]]
        self.assertEqual(removals, [f"{stack}_mysql-data", f"{stack}_postgres-data"])
        self.assertFalse(any(call[1:2] == ["swarm"] for call in calls))

    def test_success(self):
        result, calls = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(["cargo", "test", "-p", "bbgo-backtest", "-p", "bbgo-persistence", "--test",
                       "server_sql", "--locked", "--", "--ignored"], calls)
        self.assert_cleanup(calls)

    def test_failed_tests_preserve_exit_and_cleanup(self):
        result, calls = self.invoke(MOCK_TEST_EXIT="42")
        self.assertEqual(result.returncode, 42, result.stderr)
        self.assert_cleanup(calls)

    def test_partial_deploy_cleanup(self):
        result, calls = self.invoke(MOCK_DEPLOY_EXIT="17")
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertFalse(any(call[0] == "cargo" for call in calls))
        self.assert_cleanup(calls)

    def test_remote_daemon_refused_before_deploy(self):
        result, calls = self.invoke(MOCK_ENDPOINT="tcp://remote:2375")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call[1:2] == ["stack"] for call in calls))


if __name__ == "__main__":
    unittest.main()
