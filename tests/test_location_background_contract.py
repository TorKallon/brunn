"""Keep the location wake-up bounded to the single silent-push fix."""
from pathlib import Path
import re
import unittest


class LocationBackgroundContract(unittest.TestCase):
    def test_no_continuous_location_or_phone_scheduler(self):
        root = Path(__file__).resolve().parents[1] / "apps/ios/Brunn"
        reporter = root / "Location/LocationReporter.swift"
        for path in root.rglob("*.swift"):
            source = path.read_text()
            for forbidden in (
                "startUpdatingLocation", "allowsBackgroundLocationUpdates", "BGTaskScheduler",
            ):
                self.assertNotIn(forbidden, source, str(path))
            if path == reporter:
                heartbeat = re.search(
                    r"    func handleHeartbeat\(.*?(?=\n    (?:private )?func )",
                    source, re.S,
                )
                self.assertIsNotNone(heartbeat)
                self.assertEqual(heartbeat[0].count("requestLocation()"), 1)
                source = source[:heartbeat.start()] + source[heartbeat.end():]
            self.assertNotIn("requestLocation", source, str(path))


if __name__ == "__main__":
    unittest.main()
