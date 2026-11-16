"""Offline safeguards for the user-space SQL harness (no DB servers needed)."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
import test_sql_local as harness


class LocalScript(unittest.TestCase):
    def test_cleanup_on_test_failure(self):
        process = Mock()
        process.poll.return_value = None
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            with patch.object(harness.subprocess, "Popen", return_value=process):
                with self.assertRaisesRegex(RuntimeError, "test failed"):
                    with harness.server(["mysqld"], Path(directory) / "log", {}):
                        raise RuntimeError("test failed")
            process.terminate.assert_called_once()
            process.wait.assert_called_once_with(timeout=30)

    def test_unresponsive_child_is_killed(self):
        process = Mock()
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired("server", 30), 0]
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            with patch.object(harness.subprocess, "Popen", return_value=process):
                with harness.server(["postgres"], Path(directory) / "log", {}):
                    pass
        process.kill.assert_called_once()
        self.assertEqual(process.wait.call_count, 2)

    def test_bootstrap_refuses_existing_directory(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            with patch.object(harness, "run") as run:
                with self.assertRaises((FileExistsError, RuntimeError)):
                    harness.bootstrap(Path(directory))
                run.assert_not_called()

    def test_dead_server_logs_redact_password(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            log = Path(directory) / "log"
            log.write_text("failed query with secret-test-password")
            process = Mock()
            process.poll.return_value = 1
            with self.assertRaises(RuntimeError) as error:
                harness.ready(process, [], {"MYSQL_PWD": "secret-test-password"}, log)
            self.assertNotIn("secret-test-password", str(error.exception))
            self.assertIn("[redacted]", str(error.exception))


if __name__ == "__main__":
    unittest.main()
