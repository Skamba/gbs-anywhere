import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch, MagicMock

spec = importlib.util.spec_from_file_location("serial_log", Path(__file__).resolve().parents[1] / "tools/serial_log.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class OwnershipTest(unittest.TestCase):
    def test_serial_open_follows_idf_no_reset_order(self):
        events = []
        class Port:
            def __setattr__(self, key, value):
                events.append((key, value))
            def open(self):
                events.append(("open", None))
        module.open_without_reset(Port())
        self.assertEqual(events, [("dtr", True), ("rts", True), ("open", None), ("rts", False), ("dtr", False)])

    def test_running_computer_blocks_takeover(self):
        with patch.object(module.socket, "create_connection", return_value=MagicMock()):
            self.assertTrue(module.computer_running())

    def test_closed_port_allows_takeover(self):
        with patch.object(module.socket, "create_connection", side_effect=ConnectionRefusedError):
            self.assertFalse(module.computer_running())

    def test_unknown_network_error_fails_closed(self):
        with patch.object(module.socket, "create_connection", side_effect=TimeoutError):
            with self.assertRaises(RuntimeError):
                module.computer_running()


if __name__ == "__main__":
    unittest.main()
