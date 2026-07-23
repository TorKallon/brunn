Created: 2026-05-30 09:55 PDT
Updated: 2026-06-01
Status: Active
Health: cabling validated to 5G; current topology and top-shelf photo baseline captured; ready for backbone upgrade planning

Related: [[Areas/Home/Home|Home]], [[Active projects]], [[Areas/Home/UpNote imports/SE 33rd Court Ethernet Jacks|SE 33rd Court Ethernet Jacks]], [[Projects/Operations/Nyx and Tachi hardware comparison 2026-05-22|Nyx and Tachi hardware comparison]], [[Projects/Operations/Nyx Mac mini storage expansion research 2026-05-21|Nyx Mac mini storage expansion research]], [[Projects/Operations/Mac mini vs Mac Studio Nyx successor comparison 2026-06-01|Mac mini vs Mac Studio Nyx successor comparison]]

## Purpose

Plan and execute the home network buildout: UCG-Fiber migration, switching, WAN/load balancing, cabling, PoE/thermal design, WiFi tuning, NAS/storage, cameras, and related closet/garage wiring.

Search keywords: home network, homelab, UniFi, UCG-Fiber, Fiber Gateway, Comcast, fiber WAN, load balancing, USW-Pro-XG-8-PoE, Pro XG 8 PoE, PoE, TrueNAS, Protect, cameras, Nyx, Tachi, garage Cat5e, MBR closet.

## Current Status

The UniFi Network controller migration from Cloud Key Gen2 to UCG-Fiber is done. Devices came online after using Override Inform Host on the old Cloud Key.

2026-06-01 field update: the office, downstairs kids' playroom, top-floor AP location, and guest room TV room drops all tested clean to the MBR closet at 5 Gbps using the UGREEN USB adapter setup. The structured wiring cabinet now has a hinged vented door, improving access and airflow; airflow is still constrained because the cabinet sits behind suits in protective garment bags.

2026-06-01 current topology update: Comcast cable is 2 Gbps down / 400 Mbps up, and Quantum fiber is 1 Gbps symmetric. Both WANs come into the UniFi Fiber Gateway. The gateway currently feeds a Leviton 8-port 1 Gbps switch inside the structured wiring cabinet and a separate 1 Gbps UniFi switch on top of the cabinet. Almost all current switching/backbone is still 1 Gbps even though several end nodes and validated cable runs can go faster.

2026-06-01 top-shelf photo audit: the current shelf is functional but physically messy. The 5G backup hardware is present but not set up yet. The next physical step should be labeling, shelf zoning, shorter patch cables, and a power/cable-management pass before adding the 10G core switch.

2026-06-01 growth-plan update: near-term shelf design needs to reserve space for at least one more OWC external drive, one more server-class Mac mini or Mac Studio, and a switch change to `USW-Pro-XG-8-PoE`. Longer term, reserve a lower/heavier zone for a UPS and a NAS appliance rather than filling the whole top shelf with loose devices.

2026-06-01 purchasing update: the basic shelf cleanup/cable-management cart was ordered, with a slightly different Ethernet-cable mix. The desk AP decision is deferred until the realistic desk requirements are clearer. Nyx NIC decision is still open: compare the current 5GbE USB adapter against a true Thunderbolt 10GbE adapter once Nyx's actual built-in Ethernet tier is confirmed.

2026-06-04 arrival update: the Amazon shelf/garage cleanup order has arrived. The exact itemized cart is not captured in the vault yet; treat the arrived supplies as installation-ready materials for top-shelf zoning, cable labeling, rear power/Ethernet routing, short patch replacement, gateway NVR storage install prep, and garage ONT/Cat5e demarc cleanup.

2026-06-04 monitor-riser note: the monitor risers in the cleanup order are the black 2-pack LOTEYIKE metal mesh risers, about 14.6 x 9.3 x 5.5 inches each, with adjustable legs. They are intended as small freestanding equipment shelves for the top closet shelf. Use them to create a two-level compute/storage or expansion bay, keep small external drives/hubs/cable slack from sprawling across the main shelf, and preserve airflow/serviceability. Do not use them to bury hot gear, hide switch ports, stack AP/cellular radios, or support heavy UPS/NAS hardware.

2026-06-04 power-strip selection update: measure the actual routed cord distance from the wall outlet to the intended rear/right shelf mounting point before buying. The durable target is one listed, mountable surge strip plugged directly into the wall, not a daisy chain or extension-cord workaround. Prefer the Cable Matters 8-outlet strip if its 8 ft cord reaches; use a 15 ft cord model only if the measured route needs it and the longer body can mount cleanly without consuming service space.

The main remaining buildout is physical and purchasing:

- buy/install the closet switch
- establish a 10G gateway-to-closet uplink
- decide Nyx endpoint NIC: built-in Ethernet if 10GbE, current USB 5GbE adapter as a lower-heat interim path, or Thunderbolt 10GbE adapter for sustained local NAS/server work
- decide subnet strategy and DHCP reservations
- bring Nyx back onto the correct network if still stranded
- install the ordered 2TB gateway NVR SSD after ordering the tray
- plan the camera replacement
- spec the NAS direction

## Current Physical Topology - 2026-06-01

WAN:

- Comcast cable: 2 Gbps down / 400 Mbps up.
- Quantum fiber: 1 Gbps symmetric.
- Both WANs terminate at the UniFi Fiber Gateway.

Downstream from the gateway:

- One wire runs down into the structured wiring cabinet to a Leviton 8-port 1 Gbps switch.
- The Leviton switch is exactly the right port count for the current in-cabinet drops.
- Another wire runs to a 1 Gbps UniFi switch sitting on top of the cabinet.
- The top-of-cabinet UniFi switch is half PoE and half non-PoE.
- The rest of the current network is effectively 1 Gbps switching/backbone until the end nodes.
- Some end nodes can support 5 Gbps or 10 Gbps, but the current switches bottleneck them.

AP inventory note:

- Current field belief: access points are mostly UniFi WiFi 5 generation, with possibly one WiFi 6-generation AP or pack.
- Earlier controller migration inventory listed Guest Room U6 Mesh, Playroom U6 Mesh, MBR Closet FlexHD, and Top Floor U6 LR.
- Verify the controller inventory before buying APs or assigning faster PoE ports.
- Current WiFi 5 / WiFi 6 APs are mostly 1G-uplink class; new WiFi 7 APs may need 2.5G or 10G uplinks depending on model.

Gateway storage:

- A 2TB SSD has been ordered for the Fiber Gateway.
- The add-your-own-drive install still needs `UACC-SSD-Tray`.

## Top Shelf Photo Baseline - 2026-06-01

Source photos are saved next to this project note:

- [top-shelf-01](Photos/2026-06-01-top-shelf-01.jpg) - wide, left-to-right shelf context.
- [top-shelf-02](Photos/2026-06-01-top-shelf-02.jpg) - right side with UCG-Fiber, Nighthawk, power strips.
- [top-shelf-03](Photos/2026-06-01-top-shelf-03.jpg) - wide shelf view from the left.
- [top-shelf-04](Photos/2026-06-01-top-shelf-04.jpg) - compute/storage/AP cluster.
- [top-shelf-05](Photos/2026-06-01-top-shelf-05.jpg) - compute/storage/AP cluster, lower-res export.
- [top-shelf-06](Photos/2026-06-01-top-shelf-06.jpg) - Leviton structured-media cabinet door.

Observed physical setup:

- Location is the MBR closet top shelf above clothes, with a sloped ceiling/wall and limited working height.
- The Leviton structured-media cabinet is below/right with the newer hinged vented door.
- UniFi Fiber Gateway / UCG-Fiber is on the right side of the top shelf, with front display visible.
- Netgear Nighthawk device is standing upright near the right rear; this is the likely 5G backup hardware, but it is not set up yet.
- Current UniFi 1G switch is visible on the top shelf with blue ring light.
- Small metal Ethernet switch is visible near the center with yellow/white patch leads; verify whether it is active, temporary, or redundant once the core switch is installed.
- MBR Closet UniFi FlexHD-style cylindrical AP is standing on the shelf. Keep it upright and away from metal, power bricks, and stacked electronics.
- Mac mini / Nyx-class compute box, OWC Express 1M2 external NVMe enclosure, G-Drive external disk, and a black mini-PC / Beelink-class box are visible in the compute/storage cluster. Verify exact host names and which storage belongs to which host before recabling.
- A white puck-shaped device is sitting on the black mini-PC. Verify whether it is an AP/sensor/other accessory; do not leave RF or heat-producing gear stacked on top of another device.
- Power is split across rear/right power strips with multiple wall warts and loose service loops.
- Cable slack is mostly unmanaged: Ethernet and power cross over devices, service loops are in front of gear, and several cables would have to be traced by hand before maintenance.

Current maintenance risk:

- A failure or move would require cable tracing by sight and trial.
- The gateway, AP, switches, and compute/storage gear are visually mixed together instead of separated by role.
- Long patch cables are being used for short shelf hops.
- Power bricks and Ethernet are interleaved, making it harder to isolate a network issue from a power issue.
- Stacked or close-packed gear may block airflow or RF, especially around the AP and 5G backup hardware.

## Target Top Shelf Organization

Design goal: make the shelf serviceable without turning it into a rack. The structured-media cabinet stays passive/low-speed; the open shelf becomes the maintainable core.

Recommended zones, left to right, adjusted after checking 5G signal:

1. Radio / cellular edge: 5G backup hardware and the MBR AP, kept upright, unstacked, and separated from metal/power bricks. If the closet has weak 5G signal, move the 5G device to the best-signal location and run Ethernet back to the gateway instead of forcing it to live on this shelf.
2. Compute/storage bay: Nyx/Mac mini, OWC Express 1M2, G-Drive, and black mini-PC-class box. Keep storage cables short and local to the compute device they serve. Do not put APs or cellular devices on top of compute boxes.
3. Core network bay: UCG-Fiber plus the future `USW-Pro-XG-8-PoE`, close enough for the short SFP+ DAC. Put the switch where ports and labels can be inspected from the closet aisle.
4. Expansion bay: leave open, ventilated shelf width for one more OWC external drive and one more server-class Mac mini or Mac Studio. Do not consume this space with permanent cable slack.
5. Power bay: one labeled UPS/surge-fed power strip at the right/rear or lower cubby, with large wall warts secured so they do not occupy the working surface.
6. Future lower/heavy bay: UPS and NAS appliance should probably live below the top shelf or in a lower cubby if dimensions and ventilation allow. Avoid putting a NAS full of disks high on the top shelf unless the shelf load rating and vibration behavior are known.

Cable-management pattern:

- Add two rear cable paths: one for power and one for Ethernet. Use an adhesive raceway, under-shelf cable tray, or Velcro tie mounts along the back edge.
- Keep service loops at the rear, not draped over devices.
- Replace shelf-hop Ethernet with short slim Cat6/Cat6A patch cables where possible: 0.5 ft, 1 ft, 2 ft, and 3 ft should cover most shelf runs.
- Label both ends of every network cable before unplugging anything. Minimum labels: `WAN-Comcast`, `WAN-Quantum`, `WAN-5G`, `UCG-to-core-10G`, `Leviton-1G`, `top-UniFi-1G`, `MBR-AP`, `Nyx`, and `Tachi` if present.
- Color-code only by role, not by random cable availability: WANs one color family, core/uplinks another, access/AP drops another, compute/storage another.
- Leave each device removable without cutting ties. Use Velcro, not tight zip ties, for active cables.
- Take an after photo and keep it with this note once the shelf is cleaned up.
- Leave at least two empty AC outlets and two clean Ethernet paths for the next external drive/server before calling the cleanup done.

5G backup integration notes:

- Before choosing a permanent 5G location, test signal and throughput in the current shelf spot and at any nearby better RF spot.
- If keeping Comcast plus Quantum plus 5G, verify the current UniFi Network version and gateway behavior for a third WAN / failover path before relying on it.
- Preferred role for 5G is emergency failover, not normal load-balanced traffic.
- Put gateway, core switch, AP, and 5G backup on UPS-backed power if the goal is real outage resilience.

## Existing Measurements and Constraints

These came from the earlier 2026-05-30 discovery note and should stay with the project.

- MBR/master bedroom closet shelf depth: 14 inches.
- Treat 14 inches as the hard shelf-depth constraint for switch sizing; leave extra clearance for Ethernet cables, power, and airflow.
- In-wall switch location/device size: 3.5 inches wide by 6 inches tall.
- Width is the hard constraint: 3.5 inches cannot get much bigger.
- Height is more flexible: 6 inches could likely become 8-10 inches if needed.
- Longest measured cable run so far: 96 inches.
- Initial cable list: 3 x 8-foot cables, 1 longer cable, 2 x 6-foot cables, plus shorter patch cables.
- Minimum known length is already 36 feet before the one longer run and shorter patch cables.
- Planning target: buy/cut about 50 feet unless the one longer run is much over 10 feet; use 60 feet if extra slack and mistake tolerance are worth it.

Existing patch panel / cable mapping lives in [[Areas/Home/UpNote imports/SE 33rd Court Ethernet Jacks|SE 33rd Court Ethernet Jacks]].

Previous termination note: the vault does not currently identify T568A vs T568B. If an existing cable works, match the other end; otherwise T568B is the common default.

## 1. WAN and Gateway

Hardware: UniFi UCG-Fiber, also called Fiber Gateway. LAN is at `192.168.1.1`.

Ports:

- 2 x 10G SFP+
- 1 x 10GbE RJ45
- 4 x 2.5GbE
- Every port role is reconfigurable.

Two ISPs:

- Comcast cable: 2 Gbps down / 400 Mbps up.
- Quantum fiber: 1 Gbps symmetric, enters in the garage.
- Gateway is set to load balance.

Core bottleneck identified early: the gateway is the most capable device in the chain, but the 1 Gbps switches were funneling all roughly 3 Gbps of WAN through a single 1 Gbps gateway-to-closet link. The fix is a multi-gig uplink, not a faster gateway.

Recommended port assignment:

- Comcast 2G -> 2.5GbE port set as WAN.
- Fiber 1G -> 2.5GbE port set as WAN.
- SFP+ #1 -> 10G uplink to the closet switch.
- 10GBASE-T RJ45 -> Nyx directly at native 10G if Nyx truly has the 10GbE option.
- SFP+ #2 -> spare 10G for future NAS or 10G run.

Important verification: this pasted planning note says Nyx has native 10GBASE-T, but the older [[Projects/Operations/Nyx and Tachi hardware comparison 2026-05-22|Nyx hardware snapshot]] captured order info listing Gigabit Ethernet. Verify the actual Ethernet tier before making Nyx the direct 10G RJ45 endpoint.

Load balancing behavior:

- UniFi has Failover and Load Balancing / Distributed modes.
- Distributed mode uses per-session connection hashing on source/destination IP; a session stays on one WAN.
- Weights, such as 80/20, are session-count ratios, not bandwidth-aware.
- Load balancing is not bonding. A single flow/download never exceeds one WAN, and a speed test normally touches only one WAN.
- Egress public IP flips per session, which can break IP-pinned services such as banking logouts, captchas, and streaming/account checks.
- Inbound/hosted services realistically live behind one WAN, with port-forward plus DDNS pointed at one IP.

Recommended WAN strategy: use policy-based routing.

- Pin upload-heavy, latency-sensitive, and inbound-serving traffic to fiber: servers, Discord bots, VoIP, and gaming.
- Let bulk downloads lean on Comcast.
- Load-balance generic browsing.
- Keep outbound-serving traffic on fiber unless Comcast's 400 Mbps upstream is explicitly acceptable for the service.

Prior open item: fiber WAN was not showing in ISP Health, likely because the fiber's blue Cat5e run from the garage was not cleanly connected. Current field update says Quantum fiber is now part of the live gateway setup; verify ISP Health before treating this as fully closed.

## 2. Switching

Closet switch chosen direction as of 2026-06-01 growth-plan update: `USW-Pro-XG-8-PoE`, also called Switch Pro XG 8 PoE, with 155W PoE budget.

Reasons:

- 8 x 10GbE RJ45 PoE++.
- 2 x 10G SFP+.
- Compact desktop/wall-mountable form factor.
- Gives every high-value wired shelf/server/AP uplink a 10G-capable copper port.
- Avoids immediately needing 10G copper SFP+ modules for local shelf devices.

Caveats:

- This is a hotter, denser 10GBASE-T switch than the earlier `USW-Pro-Max-16-PoE` plan.
- Treat airflow as a first-class requirement: do not bury it behind wall warts or cable coils.
- It has fewer RJ45 ports than the 16-port plan, so the Leviton 1G in-cabinet switch remains useful for low-speed access drops.
- 10G copper ports are convenient but thermally expensive; use the SFP+ DAC for the gateway uplink and reserve RJ45 10G for devices that actually need it.

Earlier switch direction retained for context: `USW-Pro-Max-16-PoE` with 180W PoE budget.

Why it was attractive:

- Fanless/silent.
- 4 x 2.5G PoE++.
- 12 x 1G PoE+.
- 2 x 10G SFP+.
- About 160 mm deep, so it fits shallow cabinets.
- Pro Max 24/48 have fans; 16-PoE does not.
- It was a reasonable lower-heat choice if the goal was mostly 2.5G AP uplinks plus many 1G ports.

In-wall switch options if ever needed:

- `USW-Flex-2.5G-8 PoE`: 8 x 2.5G plus 10G SFP+/RJ45 combo uplink, fanless, PoE.
- `USW-Flex-2.5G-5` / Flex Mini 2.5G: 5 x 2.5G, fanless, USB-C or PoE-in, cooler, fits structured-media enclosures with brackets. No SFP+; uplink is a 2.5G port.

SFP+ connectivity facts:

- SFP+ ports are empty cages and need a DAC cable or transceiver.
- Bought or identified: `UACC-DAC-SFP10-1M` for gateway-to-closet switch. DAC is cheapest and coolest for short runs.
- UniFi DAC tops at 3 m; generic DAC can reach about 5 m.
- Longer 10G runs should use fiber plus two optical SFP+ modules.
- Landing a native 10GBASE-T RJ45 device on an SFP+ port needs a copper module such as `UACC-CM-RJ45-MG` or `UF-RJ45-10G`.
- Copper SFP+ RJ45 modules run very hot; avoid where possible.
- UniFi does not lock third-party SFP/DAC; generic FS.com-type modules usually work and cost far less.

Heat ranking for multi-gig switching:

1. PoE delivery is dominant.
2. 10GBASE-T copper PHYs are the hottest port type.
3. Switch ASIC.
4. 2.5G copper PHYs are moderate.
5. SFP+ optical/DAC is cool and low-power.

Staying on 2.5G copper plus SFP+ uplinks keeps heat moderate; full 10GBASE-T copper is what runs genuinely hot.

## 3. Devices and Host Names

- Nyx: Mac Mini workhorse. Pasted plan says base M4, 24 GB, 10GbE native RJ45, Thunderbolt 4. Verify actual Ethernet tier because the earlier Operations note says Gigabit Ethernet.
- Tachi: Ubuntu mini PC server. Has two Intel I226-V Ethernet controllers in the older hardware note; pasted plan says two bindable NIC ports and asks to confirm whether they are 2.5G or 1G.
- Future: Mac Studio or stronger Mac mini with TB5 and 10GbE to separate always-on workloads from the experimental devbox; see [[Projects/Operations/Mac mini vs Mac Studio Nyx successor comparison 2026-06-01|Mac mini vs Mac Studio Nyx successor comparison]].
- Near-term future server: likely another Mac Studio or Mac mini on this shelf. Reserve one 10G-capable switch port, one UPS-backed power outlet, and physical airflow for it now.
- Near-term future storage: at least one more OWC external drive is expected, but the attached machine is not yet decided. Keep external-drive placement flexible and avoid routing storage cables through the main network cable path.
- Further-out future: UPS and NAS appliance should be planned as heavier, heat/noise-producing infrastructure. Prefer a lower cubby or dedicated ventilated shelf zone over crowding the top shelf.

LAG reality:

- Bonding aggregates bandwidth across multiple simultaneous flows, not within a single flow.
- One TCP stream rides one physical link.
- 2 x 2.5G is about 2.5G single-flow / 5G aggregate.
- 2 x 1G is about 1G single-flow / 2G aggregate.

10G is only as useful as the far end. Nyx's 10G, if present, shines when serving many clients or talking to a future true-10G host. One-to-one with a bonded-gigabit server is capped at the server's link.

## 4. Controller Migration - Done

Process used:

1. Back up the Cloud Key's UniFi Network.
2. Restore on the gateway under Settings -> Control Plane -> Backups.
3. Gateway's Network version must be at least the Cloud Key version.
4. Restore recreated networks, VLANs, SSIDs, firewall, and DHCP.
5. Devices initially showed offline because their inform host still pointed at the Cloud Key.
6. Working fix: enable Override Inform Host on the Cloud Key and set it to the gateway's LAN IP.
7. Devices migrated and came online on the gateway.

Fallback: SSH to a device and run `set-inform http://<gw-ip>:8080/inform`.

Override Inform Host is transitional and should not be needed afterward.

Safe Cloud Key retirement steps:

- Unplug Cloud Key once all devices are online/provisioned/stable.
- Forget the Cloud Key's devices.
- Remove the Cloud Key from the device list / Site Manager.
- Watch for stale DHCP option 43 pointing at the old Cloud Key IP.

Fleet at migration:

- 4 APs: Guest Room U6 Mesh, Playroom U6 Mesh, MBR Closet FlexHD, Top Floor U6 LR.
- 1 switch: US-8-60W, gigabit.
- UCG-Fiber gateway.
- In-wall second switch is non-UniFi / unmanaged.

This inventory conflicts with the later field belief that APs are mostly UniFi 5 generation with possibly one UniFi 6-generation AP/pack. Treat controller inventory as the next source of truth to recheck.

## 5. Subnet and IP Lessons

Old LAN was `192.168.175.0/24`. Servers had static IPs there.

The new gateway came up on `192.168.1.0/24`, stranding static/old-subnet devices.

Tailscale nodes went dark because they lost internet. Overlay reconnects automatically once a node has a working path; node identity persists and no re-auth should be needed. Laptops on DHCP renewed; Nyx, with static or stale lease, did not.

The "fixed IP is invalid" UniFi app error came from entering `192.168.175.115` on a `192.168.1.0/24` network.

Two clean options:

1. Renumber static devices to `192.168.1.x`; cleaner long-term.
2. Change the gateway LAN back to `192.168.175.0/24`; zero per-device edits and instantly un-strands static devices such as Nyx.

If the subnet changes, re-advertise Tailscale subnet routes.

Remote-reboot Nyx without monitor/keyboard:

- Hold power button about 10 seconds to force off, then press to power on.
- This only fixes Nyx if it is DHCP/stale lease. A manual static config survives reboot.
- Better: temporarily set a laptop to `192.168.175.x`, SSH to `192.168.175.115`, then run `networksetup -setdhcp Ethernet` or fix the config.
- Alternate zero-touch path: change gateway subnet so Nyx's static config becomes valid again.

Durable fix: use controller-owned DHCP reservations instead of client-side static IPs, so future gateway changes cannot strand devices.

## 6. WiFi, AP Density, and IDS

AP Deployment Density flag on 5 GHz is advisory/noisy. Ubiquiti does not publish the logic and it can fire on healthy networks. It recomputes after migration/reboots; wait a day before reacting. WiFi Experience was 99%/97%, so it was not urgent.

Channel tuning:

- Enable DFS channels on 5 GHz. This is the biggest lever; it gives 4 APs enough channels to avoid reuse/collision. UniFi disables extra DFS channels by default.
- Set 5 GHz width to 40 MHz, not 80/160, for better channel reuse.
- Lower TX power on APs that sit close together.
- Run Channel Optimization, called Channel AI in newer builds.
- Settings path: Settings -> WiFi; per-radio channel/width under Settings -> Radios.
- Auto can take about 24 hours to settle.
- With only 4 APs, manually assigning 4 non-overlapping 5 GHz channels is deterministic and fine.

IDS/IPS false positive:

- Nyx -> AWS was flagged as a threat.
- Suppress via threat detail -> Allow Signature, which adds to Signature Suppression under Settings -> Security -> Intrusion Prevention.
- Or Exclude Source IP.
- Check mode: Detect Only is IDS and only alerts; Detect and Block is IPS and blocks the source for 300 seconds.
- If in IPS mode, the false positive may be intermittently cutting Nyx off from AWS.
- Pair any IP-based exclusion with a DHCP reservation for Nyx.

## 7. Storage, NAS, and Backup

Primary direction: DIY TrueNAS SCALE with ZFS.

Why it fits:

- Backups: Time Machine targets over SMB, with per-machine quotas for all Macs.
- Media, mostly video: ZFS HDD pool using RAIDZ2 or mirrors.
- Photos/videos: Immich container for photo/video library. Synology Photos is the turnkey equivalent.
- Block storage: ZFS zvol to iSCSI or NVMe/TCP in SCALE 25.10+, with snapshots/clones as EBS-like semantics.
- Offsite 3-2-1: `zfs send` replication, or `restic`/`rclone` to Cloudflare R2 or Backblaze B2.

Key caveats:

- macOS has no native iSCSI/NVMe-oF initiator; it needs globalSAN/ATTO.
- Linux/Tachi is first-class for block storage.
- Serve block to Linux and files over SMB/NFS to Macs.
- RAID is not backup; snapshots are not backup because they are on the same pool.
- Precious media needs an offsite copy.
- Apple Silicon does not support bootable clones in the old sense. Recovery means reinstall macOS and migrate. Time Machine or CCC data backup to NAS is the model.

Prebuilt alternatives:

- Synology: drive-lock reversed in DSM 7.3 for HDD/SATA SSD, but M.2 still locked and hardware is aging.
- UGREEN NASync: good value and can run TrueNAS.
- QNAP: strong iSCSI and Thunderbolt models, but has a security-history caveat.

## 8. Mac External Storage - Nyx and Future Studio

Real-world speed tiers:

| Path | Real-world ceiling |
|---|---:|
| Internal NVMe, Gen3/4 | about 3,500 / 7,000 MB/s |
| TB5 external NVMe | about 6,000-6,300 MB/s |
| TB4 / USB4 external NVMe | about 3,000-3,400 MB/s |
| USB 10Gb SSD | about 1,000 MB/s |
| 10GbE network | about 1,100 MB/s |
| HDD per drive | about 250-270 MB/s |

Economics:

- Apple internal BTO: about $0.25-$0.50/GB; avoid when possible.
- NVMe: about $0.05-$0.08/GB.
- HDD: about $0.015-$0.02/GB.

Decisions:

- Nyx is Thunderbolt 4 if it is base M4, so external storage caps around 3.4 GB/s.
- A TB5 enclosure runs at TB4 speed on Nyx and is only worth buying as future-proofing for a Studio.
- On TB4, a flagship drive like 990 Pro is bottlenecked; a cheaper mid-tier TLC+DRAM drive performs similarly.
- Plug external enclosures into rear Thunderbolt ports. Front USB-C is only 10 Gbps.
- SanDisk Extreme 4TB E61 USB 10Gb, about 1,050 MB/s, is worse for a stationary always-on box than OWC + NVMe: OWC route gives about 3x speed, better reliability, and replaceable drive.
- SanDisk's rugged/portable features are wasted on a fixed server, and the Extreme line has 2023 data-loss reputation plus sustained-write throttling concerns.
- Recommended TB5 enclosure for future Studio: `OWC Express 1M2 (80G)` single-slot DIY, best Mac support, silent/passive.
- Alternative: Acasis 80Gbps / TB501 Pro, fan, cheaper, better sustained thermals.
- Pair with fast Gen4 TLC+DRAM drive such as SN850X or 990 Pro; this only pays off on a TB5 host.

This external-storage decision is separate from the gateway's NVR SSD.

## 9. Cameras - Replacing Nest with UniFi Protect

Why Protect:

- Local-first recording to gateway plus SSD.
- No subscription.
- Footage stays on the LAN instead of Google cloud/Nest Aware.

Facts:

- The UCG-Fiber plus its M.2 SSD is the NVR for this setup; no separate NVR box is needed.
- UniFi WiFi cameras exist in the Instant line: G6 Instant 4K and G4 Instant 2K.
- WiFi cameras are USB-powered. WiFi solves data, not power.
- Instant cameras are indoor only.
- UniFi makes no battery/solar cameras.
- ONVIF support in Protect 3.0+ lets third-party cameras record locally to Protect. This usually loses AI/two-way but keeps recording.

Plan for 2 cameras:

- The most-private indoor camera is next to an Ethernet drop, so use wired PoE if possible, such as G5 Flex or G5/G6 Turret.
- Other spots not near drops: WiFi Instant if there is an indoor outlet, short cable run / PoE-over-powerline, or third-party ONVIF camera for no-power/no-Ethernet spots.
- WiFi cameras add airtime load to APs; wired PoE is more reliable for 24/7.

## 10. Gateway NVR SSD

The UCG-Fiber takes an NVMe in a proprietary tray. It uses the `UACC-SSD-Tray`. The tray is not included when adding a drive later; buy tray, SSD, and opening tool. Storage extends logging/event retention and enables Protect.

Decision: 2TB SSD ordered. Previous recommendation was `WD Black SN850X`, 2TB, no-heatsink version.

Rationale:

- SN850X is TLC plus DRAM, the two traits that matter here.
- Cheaper than 990 Pro / SN700.
- Heat is a non-issue at NVR write rates of roughly 200-500 MB/s, because the drive is loafing at about 5% of capacity.
- "Runs hot" mainly applies under benchmark loads.
- Buy the bare no-heatsink version; the heatsink model will not fit the tray.
- WD Red SN700's endurance edge is real but academic for a 2-camera home NVR and costs more.

Capacity:

- With only 2 cameras, 2TB is the comfortable sweet spot.
- 1TB is fine for motion-only recording.
- 4TB only starts making sense with more cameras.

Class rules: TLC + DRAM, M.2 2280, bare, Gen3 is fine. Avoid QLC/DRAM-less budget drives. If the ordered SSD is not the SN850X, verify it is bare M.2 2280 NVMe and does not have a heatsink before install.

## 11. Garage Fiber Demarcation and Cabling

Layout:

- Fiber enters the garage.
- Lumen Q1000K ONT has yellow fiber in and converts to Ethernet.
- Blue Cat5e carries the fiber-WAN Ethernet to the MBR closet gateway.
- Cable Matters box is a surface-mount/coupler to clean up the dangling RJ45.

Decisions/guidance:

- Mount the Cable Matters box directly under the ONT as a clean junction.
- Keep it away from the 240V outlet.
- This is an exterior wall with studs, so fishing horizontally is hard and would mean drilling studs or disturbing insulation. Builder used staples.
- Clean options: vertical within one stud bay into attic/crawl, travel in unfinished space, then drop into the closet wall; or use paintable surface raceway such as Wiremold/CordMate.
- Do not route network into the 240V box; keep low-voltage separate from line-voltage.
- Do not staple/kink fiber. Maintain gentle bend radius and a loose loop at the ONT. Only Cat5e travels.
- Historical note: properly connecting the blue run was the likely fix for the formerly missing fiber WAN; verify ISP Health now that Quantum is part of the live gateway setup.

## 12. Cabling Validation - iperf3

Method:

- 2026-06-01 field validation used the UGREEN 5Gbps USB-based adapter setup.
- Laptop-to-laptop testing with UGREEN 5GbE USB-C adapters on both ends remains the repeatable method.
- No crossover cable needed because Auto-MDI-X is standard since gigabit.
- Direct connection has no DHCP; use static IPs or APIPA link-local.
- Commands: `iperf3 -s` and `iperf3 -c <ip>`.
- Use `-P` for parallel streams and `-R` for reverse.
- The adapters cap the result; this validates clean 5G behavior, not 10G.

Results: all tested drops are valid up to the 5 Gbps adapter ceiling; iperf-style results at about 4.69 Gbps represent 5G line rate for the adapters.

- Office -> MBR closet: passed.
- Downstairs kids' playroom -> MBR closet: passed.
- Top floor AP location -> MBR closet: passed.
- Guest room TV room -> MBR closet: passed.

Conclusion:

- Cabling is proven good to 5G everywhere tested.
- Builder structured wiring and terminations are uniformly solid.
- 10G is untested and would need 10G adapters.
- Cat5e at length is a coin flip for 10G.
- Only the office-to-closet link would ever want a 10G re-test, for a future workstation-to-NAS path.

Implication:

- Cabling is not the constraint for faster APs.
- Every WAP location is cabling-ready for WiFi 7, which needs only a 2.5G uplink.
- The gate is the switch's multi-gig PoE ports plus U7 APs.
- Current U6 APs are gigabit-class and do not benefit from faster uplinks.

Where multi-gig actually matters:

1. Office to closet for single-endpoint speed.
2. AP uplinks once on WiFi 7.
3. Gateway to closet-switch backbone for whole-house WAN aggregate.
4. Future Studio-to-NAS or Nyx-to-NAS.

## 13. Closet PoE and Thermal Architecture

Done: replaced the screw-on panel on the structured-wiring box with a hinged, vented door that opens without unscrewing. This improves convenience and airflow and is good prep for the warmer multi-gig PoE switch.

Thermal notes:

- Convection works bottom-to-top.
- Intake low, exhaust high.
- Leave breathing room.
- The cabinet is still behind suits in protective garment bags, so real airflow is limited even with the vented door.
- A quiet USB fan is the easy step if heat becomes a problem.

Design decision:

- No PoE switch and no wall wart inside the structured-wiring box; avoid trapped heat.
- Main switch lives on the open top shelf, where AC power and heat have airflow.
- The structured-wiring box stays passive.
- Run 1-3 extra wires to the top shelf for future PoE. Spec these as Cat6/6a because these are controlled runs.
- Decentralized PoE: put switches at far ends of runs near device clusters rather than pushing PoE long distances.
- Do not push PoE through old builder Cat5e due to bundle heat, voltage drop, and quality uncertainty.

PoE cascade caution:

- A PoE-powered switch over a single copper run can power itself plus a light load.
- A spot that must feed several cameras/APs needs local AC.
- The limit is passthrough power budget, worsened by voltage drop over distance.
- PoE and multi-gig data coexist fine on one cable; power budget is the thing to watch.

## 14. Backbone Upgrade Plan - 2026-06-01

Goal: upgrade the backbone in layers without replacing working 1G access switching before it matters.

Stage 0 - stabilize and label the current layout:

- Label the two gateway downstream wires: Leviton in-cabinet switch and top-of-cabinet UniFi switch.
- Confirm both Comcast and Quantum show healthy in UniFi ISP Health.
- Order `UACC-SSD-Tray` and install the already-ordered 2TB gateway SSD.
- Recheck AP models in the controller and record which APs are WiFi 5, WiFi 6, or newer.

Stage 1 - create a 10G core path:

- Make the top-of-cabinet switch location the core switching point.
- Install the planned `USW-Pro-XG-8-PoE` on the open top shelf, not inside the structured wiring box.
- Use SFP+ DAC for a 10G gateway-to-core-switch uplink.
- Keep the Leviton 8-port 1G switch as an access switch for the in-cabinet low-speed drops.
- Move any high-value drops or future AP drops to the new core switch ports.
- Reserve 10G-capable ports for: gateway uplink path, Nyx if truly 10GbE, the future Mac Studio/Mac mini server, and the future NAS if it lands on this shelf.

Stage 2 - use 2.5G PoE only where it pays:

- Current WiFi 5 / WiFi 6 APs do not need faster-than-1G uplinks unless the controller inventory proves otherwise.
- Reserve 2.5G PoE ports for future WiFi 7 APs or other multi-gig PoE devices.
- Since all tested AP/drop cabling is clean to 5G, the switch/AP generation is the constraint, not the in-wall cabling.

Stage 3 - add fast endpoint paths deliberately:

- Prioritize the office-to-closet path for a 5G/10G endpoint test because it is the most likely workstation-to-NAS or workstation-to-core link.
- Use 2.5G/5G endpoints where possible before adding hot 10GBASE-T copper modules.
- If one 10G copper endpoint is needed, use the gateway's 10GbE RJ45 or one SFP+ copper module carefully.
- If multiple 10G endpoints appear, add a dedicated 10G aggregation/access layer instead of burning the only spare SFP+ port one device at a time.
- With the Pro XG 8 PoE path, treat 10G copper as available but not free: it simplifies endpoint wiring but increases switch heat and UPS load.

Stage 3b - reserve expansion capacity:

- Keep one open compute/storage footprint for the next Mac Studio/Mac mini.
- Keep one flexible external-drive footprint for the next OWC enclosure until its host is decided.
- Do not buy custom-length cables until the next server and OWC drive placement is chosen; use temporary Velcro-managed slack.
- Leave a clear future NAS path: either 10G RJ45 into the Pro XG 8 PoE or SFP+ to a future aggregation/NAS switch.

Stage 4 - AP refresh:

- Replace APs only when there is a concrete WiFi need, not just because the wired backbone is faster.
- For normal WiFi 7 APs, plan around 2.5G PoE uplinks.
- For APs with 10G uplinks, verify power and heat first; they may push the design toward a different core switch or a local injector.

Backbone principle:

- Gateway-to-core should be 10G first.
- Core-to-AP should become 2.5G when APs are replaced.
- Core-to-workstation/NAS can become 5G/10G after the endpoint exists.
- The Leviton 1G switch can stay as low-speed access until port count, management, or topology pressure says otherwise.

## Consolidated Next Actions / Shopping List

- [ ] Confirm Comcast and Quantum both show healthy in UniFi ISP Health.
- [ ] Decide subnet strategy: renumber to `192.168.1.x` or move gateway LAN to `192.168.175.0/24`.
- [ ] Convert servers to DHCP reservations.
- [ ] Get Nyx back online if still stranded. Reboot if DHCP; fix config via old-subnet SSH or gateway-subnet change if static.
- [ ] Confirm Nyx chip and actual Ethernet tier. The pasted network plan assumes 10GbE, but an older Operations note says Gigabit Ethernet.
- [ ] Confirm Tachi NIC speed, 2.5G vs 1G, to decide whether LAG is worth two switch ports.
- [ ] Label the two gateway downstream wires: Leviton in-cabinet switch and top-of-cabinet UniFi switch.
- [ ] Label all visible top-shelf network and power leads before recabling.
- [ ] Verify the small metal Ethernet switch's role and remove it if it becomes redundant after the core switch install.
- [ ] Verify the white puck-shaped device sitting on the black mini-PC and move it off stacked electronics if it is RF, sensor, or heat-producing gear.
- [ ] Test 5G backup signal on the shelf and at nearby better-signal positions before deciding where the Nighthawk/cellular device permanently lives.
- [ ] Verify UniFi support/config behavior for Comcast + Quantum + 5G as three WAN/failover paths.
- [ ] Buy/install basic cable management: rear raceway or under-shelf tray, Velcro tie mounts, cable labels, and short slim Cat6/Cat6A patch cables.
- [ ] Create the target shelf zones: radio/cellular, compute/storage, core network, and power.
- [ ] Reserve top-shelf expansion space for one more OWC external drive and one more Mac Studio/Mac mini-class server.
- [ ] Verify AP inventory in UniFi controller; reconcile current field belief with earlier U6/FlexHD inventory.
- [ ] Buy `UACC-DAC-SFP10-1M` for the gateway-to-closet 10G uplink if not already purchased.
- [ ] Buy/install `USW-Pro-XG-8-PoE` and plan extra airflow/UPS load for its 10G copper ports.
- [ ] Gateway NVR storage: order `UACC-SSD-Tray`; install the ordered 2TB SSD after confirming it is bare M.2 NVMe and fits the tray.
- [ ] Cameras: plan the wired-PoE camera by the drop; decide WiFi Instant vs ONVIF for the second camera.
- [ ] NAS: spec the TrueNAS SCALE build; set up Time Machine SMB targets, Immich, and R2/offsite backup.
- [ ] Future physical planning: decide whether UPS and NAS live in a lower closet cubby, a dedicated shelf, or a different ventilated location.
- [ ] Mac external storage: use OWC Express 1M2 80G plus a Gen4 TLC+DRAM drive only if buying for future Studio/TB5. Nyx TB4 alone cannot use TB5 speed.
- [ ] Tune WiFi: enable DFS, set 5 GHz to 40 MHz, lower TX power on close APs, and run Channel Optimization.
- [ ] Suppress the Nyx-to-AWS IDS false positive after confirming IDS vs IPS mode.
- [ ] Run 1-3 Cat6/6a cables to the top shelf while access is easy.

## Open Questions to Verify

- Does Nyx actually have 10GbE, or only Gigabit Ethernet from the older order snapshot?
- Is Nyx base M4/TB4 or M4 Pro/TB5? The 24 GB memory size exists on both.
- Are Tachi's two NIC ports 2.5G or 1G?
- Which exact AP models are currently installed, and which have only 1G uplinks?
- Is the ordered 2TB gateway SSD bare M.2 2280 NVMe without heatsink?
- Is the Cable Matters box a coupler or surface-mount keystone box?
- Is there attic/crawl access near the garage wall for a hidden vertical cable route?
- Is the gateway in IDS Detect Only or IPS Detect and Block mode?
