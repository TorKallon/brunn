Created: 2026-05-17
Updated: 2026-05-18
Status: Research note
Related: [[Projects/N24 RaceWatch/N24 RaceWatch|N24 RaceWatch]], [[Topics/F1/F1|F1]], [[Projects/Treehouse/Treehouse|Treehouse]], [[Active projects]]

# 2027 endurance calendar and timing research

Research question: which 2027 N24 / related endurance dates are already announced, and which events appear to use the same live timing WebSocket mechanism as N24 RaceWatch?

## N24 and Nordschleife races

- The ADAC RAVENOL 24h Nurburgring date is announced through 2028. The 2027 event is 27-30 May 2027.
- The ADAC 24h Nurburgring Qualifiers date is also announced. The 2027 Qualifiers are 30 April-2 May 2027.
- I did not find an official full 2027 NLS season calendar as of this check. The official NLS calendar page currently lists 2026 only.
- The 2026 official NLS calendar has ten races at eight events, with the early-season path to the 24h consisting of NLS1 on 14 March 2026, NLS2 on 21 March 2026, NLS3 on 11 April 2026, and ADAC 24h Qualifiers on 18-19 April 2026.
- Live timing compatibility: N24 and NLS are the promising same-stack targets. Official 24h and NLS live pages link to `livetiming.azurewebsites.net`. The RaceWatch collector uses `wss://livetiming.azurewebsites.net` with the subscription shape `{ eventId, eventPid, clientLocalTime }`. N24/Qualifiers use event id 50 in our current configuration; regular NLS has historically used a different event id, often 20, but the transport/app family is the same.

## 2026 NLS schedule update

Checked on 2026-05-18 after the 2026 N24 race. The RaceWatch off-season homepage should treat NLS6 on 20 June 2026 as the next covered race, not a 2027 event.

Official 2026 NLS calendar:

| Date | Race slot | Event |
| --- | --- | --- |
| 14 March 2026 | NLS1 | 71. ADAC Westfalenfahrt (4h) |
| 21 March 2026 | NLS2 | 58. ADAC Barbarossapreis (4h) |
| 11 April 2026 | NLS3 | 57. Adenauer ADAC Rundstrecken-Trophy (4h) |
| 18-19 April 2026 | NLS4+5 | ADAC 24h Qualifiers (2x4h) |
| 20 June 2026 | NLS6 | 1. ADAC Eifel Trophy (4h) |
| 1 August 2026 | NLS7 | KW 6h ADAC Ruhr-Pokal-Rennen |
| 12 September 2026 | NLS8 | 65. ADAC Reinoldus-Langstreckenrennen (4h) |
| 13 September 2026 | NLS9 | 58. ADAC Barbarossapreis (4h) |
| 10 October 2026 | NLS10 | 2. NLS Sportwarte-Trophy (4h) |

Product implication:

- Homepage: primary countdown should point to NLS6 on 20 June 2026.
- Homepage schedule: show the remaining 2026 NLS season clearly, while keeping the first five race slots as past context.
- 2027: keep ADAC 24h Qualifiers and the ADAC RAVENOL 24h Nurburgring as looking-ahead countdowns; keep 2027 NLS as TBA until the official calendar exists.
- Development workflow: make and review this first on Nyx dev at `http://nyx:5173/`; do not deploy until the user approves the dev version.

## Similar endurance races

- CrowdStrike 24 Hours of Spa: 2026 is set for 24-28 June 2026 / race start 27 June. I did not find an official 2027 date yet. Not the same N24 Azure WebSocket stack. The public SRO/Spa pages expose live timing, and TSL states it provides timing services for SRO GT World Challenge, but this is a different provider family.
- 24 Hours of Le Mans: 2026 is set for 10-14 June 2026, with the 24h race listed by FIA as 13-14 June. I did not find an official 2027 date yet. Not the same N24 Azure WebSocket stack; WEC live timing is via FIAWEC+ and public Porsche timing pages say the data is powered by Al Kamel Systems.
- Rolex 24 at Daytona: 2027 is announced for 28-31 January 2027, with the Roar Before the Rolex 24 on 22-24 January. Not the same N24 Azure WebSocket stack; IMSA timing is Al Kamel-powered.
- Mobil 1 Twelve Hours of Sebring: 2027 IMSA schedule is announced, with the event window 17-20 March 2027 and the 75th race on 20 March 2027. Same IMSA/Al Kamel family as Daytona, not the N24 Azure WebSocket stack.
- Motul Petit Le Mans: 2027 IMSA schedule is announced for 6-9 October 2027. Same IMSA/Al Kamel family, not the N24 Azure WebSocket stack.
- Meguiar's Bathurst 12 Hour: 2027 event page says 11-14 February 2027. Not the same N24 Azure WebSocket stack; Bathurst public live timing exists, and published timing/results material points to Natsoft rather than the N24 Azure stack.
- Intercontinental GT Challenge 2026: official calendar includes Bathurst 12 Hour, N24, Spa 24, Suzuka 1000km, and Indianapolis 8 Hour. I did not find a 2027 IGTC calendar yet. Except for N24, do not assume same timing transport.
- 24H Series / Dubai / Barcelona: useful comparable 24-hour events, but generally a different scale and a different timing provider family. Public 24H Series links commonly point to `livetiming.getraceresults.com/24hseries`, not `livetiming.azurewebsites.net`. I did not find a 2027 24H Series calendar in this pass.

## Product implication

- Best direct re-use target for RaceWatch live ingestion before N24 2027: 2027 ADAC 24h Qualifiers and, if announced, regular 2027 NLS races. They should be validated early with a small event-id/config probe rather than treated as guaranteed.
- Spa, Le Mans/WEC, IMSA, Bathurst, and 24H Series are good RaceWatch product candidates, but likely need provider-specific adapters. The N24 UI/insight model is reusable; the raw live timing collector is not plug-and-play outside the N24/NLS Azure timing stack.

## WebSocket / live-timing reuse read

- Direct same-code reuse: N24, ADAC 24h Qualifiers, and NLS are the only current high-confidence targets for the same public WIGE/Azure timing app that RaceWatch used. The public pages link to `livetiming.azurewebsites.net`, and RaceWatch subscribes over `wss://livetiming.azurewebsites.net` using `eventId`, `eventPid`, and `clientLocalTime`.
- Likely same architecture, new adapter: 24H Series / getraceresults public timing uses a different Time Service B.V. LiveTiming app. Its demo page enables SignalR, so a browser-realtime adapter may be possible, but it is not the WIGE/Azure event/PID protocol.
- Rich but not public plug-and-play: IMSA, WEC, and Le Mans are Al Kamel families. Public pages expose live timing views, but team/live data docs describe Al Kamel protocols, cloud/on-site feeds, credentials or purchased access, and different host/port/protocol assumptions.
- SRO/Spa/IGTC are fragmented by event/region/provider. GTWC Europe documentation points to Swiss Timing-style feeds and local Kafka/FTP variants; SRO America points to Al Kamel; TSL lists selected SRO/IGTC events. Treat Spa as an event-weekend browser-network reconnaissance target, not as confirmed reusable WebSocket infrastructure.
- Bathurst currently looks least reusable for our exact collector: the public live timing page exists, but 2026 results material points to Natsoft/Supercars rather than a public WIGE-style WebSocket.

## Rule-family compatibility with N24

Research question: which related endurance events are governed by rules close enough to N24 that RaceWatch concepts such as Code 60/FCY context, SP9/GT3 class explainers, stint/driver-time reads, pit-cycle interpretation, and multi-class traffic insights can mostly transfer?

- Best match: ADAC 24h Nürburgring Qualifiers and NLS. These are the same Nordschleife/Nürburgring ecosystem as N24. N24 is run under ADAC/DMSB supplementary regulations on the Nordschleife plus GP circuit, with DMSB approval and DMSB/FIA rule layers. DMSB circuit rules include FCY, Code 60 appendix, maximum driving time, pits, safety car, and Nordschleife-specific appendices. HH Timing also treats NLS and N24 as the same WIGE timing/rules family, with WIGE 5-sector usually used for regular NLS and WIGE 9-sector for Qualifiers/N24. Product implication: highest rule reuse, highest timing reuse.
- Strong operational cousin: 24H Series / CREVENTIC races such as Dubai and Barcelona. They are not Nordschleife/DMSB races, but the rule shape is close for a RaceWatch-style product: 24-hour endurance format, GT3/GT3-AM/GT3-PRO-AM, GT4, touring and special classes, Code 60 penalties, driver stint and driving-time rules, and class/BoP-driven strategy. Product implication: our insight model is useful with a new timing adapter and a new rules profile.
- GT3 cousins but different sporting code: CrowdStrike 24 Hours of Spa and most IGTC/SRO GT3 endurance races. They share FIA GT3/BoP/multi-driver endurance DNA with N24 SP9, but run under SRO/GTWC/IGTC event rules, with fixed stint-length logic and event-specific timing/rule implementations. Spa is closer than Le Mans or IMSA for audience and car-category explanations, but not same enough to reuse N24 pit/Code 60/Nordschleife assumptions without a separate rules profile.
- Partial cousin: Bathurst 12 Hour. It is an IGTC-style GT3-heavy endurance race with GT4/invitational support classes, but it uses event-specific Bathurst rules, Natsoft timing, different driving-time details, a 12-hour duration, and Australian/Supercars operational context. Product implication: good for GT3 endurance storytelling, weaker for direct N24 rule transfer.
- Low rule compatibility: IMSA endurance races such as Daytona, Sebring, Watkins Glen, and Petit Le Mans. They share endurance-racing logic and GTD/GTD PRO are GT cars, but the governing framework is IMSA WeatherTech regulations with GTP, LMP2, GTD PRO, and GTD classes, IMSA-specific FCY/pit procedures, and series supplementary regulations. Product implication: RaceWatch UX patterns transfer, but rules/strategy explainers must be rewritten around IMSA.
- Low rule compatibility: WEC / 24 Hours of Le Mans. Le Mans and WEC use FIA WEC/ACO sporting and technical regulations, Hypercar/LMP2/LMGT3 classes, Le Mans-specific supplementary regulations, scrutineering/Le Pesage, slow-zone logic, and Al Kamel timing. LMGT3 is based on FIA GT3, which gives a small car-category bridge, but the overall rules and race-control model are substantially different from N24. Product implication: build as a separate Le Mans/WEC profile, not as an N24 clone.

Rule-compatibility ranking for future RaceWatch reuse:

1. N24 / ADAC 24h Qualifiers / NLS - direct.
2. 24H Series - closest non-Nordschleife operational cousin.
3. Spa 24 / SRO GTWC Europe / selected IGTC GT3 races - strong GT3 cousin, separate rules.
4. Bathurst 12 Hour - GT3 cousin, event-specific and shorter.
5. IMSA endurance races - endurance UX reuse, low rule reuse.
6. WEC / Le Mans - endurance UX reuse, low rule reuse despite LMGT3 overlap.

## Sources checked

- ADAC 24h Nurburgring Termine: https://www.24h-rennen.de/termine/
- ADAC 24h Nurburgring live page: https://www.24h-rennen.de/live/
- NLS 2026 calendar: https://www.nuerburgring-langstrecken-serie.de/language/en/calendar-nurburgring-langstrecken-serie-2026/
- NLS 2026 season announcement: https://www.nuerburgring-langstrecken-serie.de/language/en/2025/09/08/ten-races-in-the-50th-season-of-the-nls/
- Nürburgring NLS tickets page: https://tickets.nuerburgring.de/p/nurburgring-langstrecken-serie?lang=en
- NLS live page: https://www.nuerburgring-langstrecken-serie.de/en/live/
- RaceWatch data-layer docs and collector config in `/Users/shared/projects/n24-racewatch`
- GT World Challenge Europe Spa event page: https://www.gt-world-challenge-europe.com/event/249/crowdstrike-24-hours-of-spa
- CrowdStrike 24 Hours of Spa live page: https://www.crowdstrike24hoursofspa.com/live
- TSL Timing homepage: https://www.tsl-timing.com/
- FIA WEC 2026 calendar: https://www.fiawec.com/en/news/2026-fia-wec-calendar-builds-on-stability-of-recent-seasons/8356
- FIA 24 Hours of Le Mans 2026 event page: https://www.fia.com/championship/events/world-endurance-championship/season-2026/24-hours-le-mans
- Porsche WEC live timing page: https://racing.porsche.com/wec/live-timing
- IMSA 2027 schedule: https://www.imsa.com/weathertech/2027-schedule/
- Daytona 2027 Rolex 24 announcement: https://www.daytonainternationalspeedway.com/2026/01/13/daytona-international-speedway-announces-2027-rolex-24-date/
- Bathurst 12 Hour 2027 event page: https://www.bathurst12hour.com.au/events/2027-bathurst-12-hour
- Bathurst 12 Hour live timing page: https://www.bathurst12hour.com.au/live-timing
- Intercontinental GT Challenge 2026 calendar: https://www.intercontinentalgtchallenge.com/calendar?filter_season_id=16
- 24H Series races: https://www.24hseries.com/races
- ADAC RAVENOL 24h Nürburgring 2026 supplementary regulations: https://www.adac-sport.com/54_ADAC_RAVENOL_24h_Nuerburgring_15407/docs/3_ADAC_RAVENOL_24h_Nrburgring_Ausschreibung_low.pdf
- DMSB Circuit Regulations 2026: https://www.dmsb.de/de/automobilsport/rundstrecke/file/282219
- HH Timing NLS & N24 rules/timing notes: https://help.hhtiming.com/series-specific-info/nls-nbr24/
- 24H Series 2026 sporting regulations draft: https://www.24hseries.com/gfx/TEAM%20INFO/01%20Regulations/2026/24H%202026%20Sporting%20regulations%20DRAFT%209%20SEPTEMBER.pdf
- 24 Hours of Le Mans regulations page: https://www.24h-lemans.com/en/lemans/regulations
- 24 Hours of Le Mans classes page: https://www.24h-lemans.com/en/lemans/classes
- IMSA 2026 sporting regulations: https://www.imsa.com/wp-content/uploads/sites/32/2026/03/11/2026-IMSA-SPORTING-REGULATIONS-and-SSR-IWSC-Blackline-031126.pdf
- HH Timing GTWC Europe notes: https://help.hhtiming.com/series-specific-info/gtwc-europe/
- HH Timing IGTC notes: https://help.hhtiming.com/series-specific-info/igtc/
- Bathurst 12 Hour classes page: https://www.bathurst12hour.com.au/the-classes
