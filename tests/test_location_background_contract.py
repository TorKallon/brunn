"""Check native background configuration; iOS tests exercise lifecycle and delivery."""
from pathlib import Path
import plistlib
import unittest


class LocationBackgroundContract(unittest.TestCase):
    def test_background_location_capability_and_single_explicit_acquisition(self):
        root = Path(__file__).resolve().parents[1] / "apps/ios/Brunn"
        source = (root / "Location/LocationReporter.swift").read_text()
        with (root / "Resources/Info.plist").open("rb") as handle:
            info = plistlib.load(handle)
        self.assertIn("location", info["UIBackgroundModes"])
        self.assertIn("remote-notification", info["UIBackgroundModes"])
        self.assertIn("CLLocationUpdate.liveUpdates(.default)", source)
        self.assertIn("CLBackgroundActivitySession()", source)
        self.assertIn("backgroundSession?.invalidate()", source)
        self.assertEqual(source.count("manager.requestLocation()"), 1)
        self.assertIn("private func requestFreshLocation(", source)
        self.assertIn("candidateReportAccuracies[$0.id]", source)
        self.assertIn("LocationTimestamp.string(from: location.timestamp)", source)

    def test_no_independent_phone_scheduler_or_parallel_legacy_gps_stream(self):
        root = Path(__file__).resolve().parents[1] / "apps/ios/Brunn"
        for path in root.rglob("*.swift"):
            source = path.read_text()
            for forbidden in ("startUpdatingLocation", "BGTaskScheduler"):
                self.assertNotIn(forbidden, source, str(path))


if __name__ == "__main__":
    unittest.main()
