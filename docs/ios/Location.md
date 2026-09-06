# Current location and visit history

Location reporting uses an iOS 17+ live location stream and a retained native
background activity session. iOS can pause sampling while stationary and resume
when movement returns. The final stationary sample is retained. Visits and
significant changes provide history and supplementary recovery. Always access
and the Location reporting control govern the session; disabling reporting
stops capture and deletes live server data.

Enable, foreground, manual refresh and silent push recovery request a bounded
fresh fix through the same protected disk queue and authenticated upload path.
Capture preserves original observation times. Uploads and reverse geocoding
are restrained independently of native sampling. Address/locality information
works without requiring an Apple visit or a preconfigured known place.
Background capture uses additional battery while moving; measure its cost
on the physical phone.

`location.presence` and `memory.open.owner_presence` expose:

- `position`: coordinates, accuracy, full observation timestamp, age and an
  approximate flag.
- `place`, locality fields and `last_seen`: derived from that position.
  `place.since` is its observation time, not an inferred arrival.
- `visit`: the separately inferred open visit, with its arrival time.
- `last_contact`: original timestamp of the latest report, not server receipt.

Fixes over 1 km accuracy cannot replace the last usable position. Fixes over
200 m are approximate and cannot establish a venue. Positions older than
15 minutes are last known. Known-place matches require the uncertainty circle
to fit inside the boundary. Reads never enrich. Raw evidence and the single
derived presence row support re-derivation; migration 0092 initializes position
from retained evidence without borrowing an old visit's label.

Silent APNs registration uses the owner's existing authenticated session and
installation endpoint, independently of alert permission and task credentials.
Tokens remain in memory on the phone and use app-bound encryption on the server.
A token for a former bundle ID must be replaced by the actual Brunn app.
Provider acceptance and a resulting usable location report are separate checks.
Push is best-effort recovery, not the primary tracking mechanism.

The Location screen shows fix time, accuracy, Maps, capture state, refresh
result and recovery registration. Content-free device health retains callback
time/source, quality category, rejection reason and upload failure. Coordinates,
labels and tokens must not appear in diagnostic logs.

## Release verification

Run location rule, endpoint, rederive, owner-presence, RLS and notification
delivery tests against a disposable database and versioned object bucket.
Run Swift core and simulator app location/push tests, then build and verify a
signed device app. Verify production revision, migration, health and hosted
presence contract.

On the phone, verify foreground fix/upload and actual-app APNs registration.
Use a locked-phone route with unfamiliar destinations, walking/driving,
stationary stops and subsequent departure. Target a usable position within
five minutes in normal conditions and explicit refresh within 30 seconds;
measure actual results and battery use. Exercise poor fixes, network loss,
permission changes and relaunch. Builds alone do not establish field recovery.

If the phone is locked, complete production deployment and the signed build.
Install if available and record any launch/unlock dependency explicitly.
Do not fabricate reports or claim unperformed field or battery measurements.
