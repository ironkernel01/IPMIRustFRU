# Executed Tier-1 conformance checks

Validation date: 11-13-2026.

NUCLEO-F401RE with one Aardvark connected to IPMB-A.

This customer report lists executed PASS/FAIL checks.

| ID | Rule/table/section | Page | Result | Detail |
|---|---|---|---|---|
| CMD-DEVICE-ID | Table 10.1.1-1; IPMI Table 20-2 | 172; IPMI 271-273 | PASS | Get Device ID fixed fields, optional auxiliary bytes, support flags and encodings |
| CMD-SELF-TEST | Table 10.1.1-1; IPMI Table 20-5 | 172; IPMI 275 | PASS | Get Self Test Results byte layout, including permitted unavailable diagnostics |
| CMD-CAPABILITIES | Table 10.1.3.1-1; Rules 4.1.4.5.1-4/-5 | 46; 183-184 | PASS | VSO identifier, Layer 1 Tier 1, interface/frequency encoding, standard/revision and FRU IDs |
| CMD-ADDRESS | §10.1.3.3; Rule 4.1.4.2-2 | 41; 186-187 | PASS | Get FRU Address Info absent/explicit FRU ID, address relation and optional trailing fields |
| CMD-ADDRESS-PHYSICAL-KEY | §10.1.3.3 | 186-187 | PASS | Physical-address lookup using the controller's own returned site/type |
| CMD-LOCATOR-ID | Rule 6.2-4; §10.1.3.11 | 123; 194 | PASS | Get Device Locator Record ID exact response and MC locator cross-reference |
| LEN-DEVICE-ID | IPMI Table 20-2; IPMI §5.3 | 271; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SELF-TEST | IPMI Table 20-5; IPMI §5.3 | 275; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SDR-INFO | IPMI Table 35-2; IPMI §5.3 | 481; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SDR-GET | IPMI Table 35-3; IPMI §5.3 | 483; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SDR-RESERVE | IPMI Table 35-4; IPMI §5.3 | 483; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SENSOR-READ | IPMI Table 35-15; IPMI §5.3 | 497; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-FRU-INFO | IPMI Table 34-2; IPMI §5.3 | 476; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-FRU-READ | IPMI Table 34-3; IPMI §5.3 | 477; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-FRU-WRITE | IPMI Table 34-4; IPMI §5.3 | 478; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-VSO-CAP | VITA §10.1.3.1; IPMI §5.3 | 183; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-IPMB-STATE | VITA §10.1.3.7; IPMI §5.3 | 189-190; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-LOCATOR-ID | VITA §10.1.3.11; IPMI §5.3 | 194; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-ADDRESS | VITA §10.1.3.3; IPMI §5.3 | 186-187; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| NA-04-00 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 00h returns C1h |
| NA-04-01 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 01h returns C1h |
| NA-04-02 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 02h returns C1h |
| NA-04-10 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 10h returns C1h |
| NA-04-11 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 11h returns C1h |
| NA-04-12 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 12h returns C1h |
| NA-04-13 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 13h returns C1h |
| NA-04-14 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 14h returns C1h |
| NA-04-15 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 15h returns C1h |
| NA-04-16 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 16h returns C1h |
| NA-04-17 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 17h returns C1h |
| NA-04-28 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 28h returns C1h |
| NA-04-29 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 29h returns C1h |
| NA-04-2A | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 2Ah returns C1h |
| NA-04-2B | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 2Bh returns C1h |
| NA-2C-01 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 01h returns C1h |
| NA-2C-02 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 02h returns C1h |
| NA-2C-03 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 03h returns C1h |
| NA-2C-1B | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Bh returns C1h |
| NA-2C-1C | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Ch returns C1h |
| NA-2C-1D | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Dh returns C1h |
| NA-2C-1F | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Fh returns C1h |
| NA-2C-20 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 20h returns C1h |
| NA-2C-21 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 21h returns C1h |
| FRUID-AREA-INFO | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-READ | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-WRITE | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-ADDRESS | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-LOCATOR | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| CMD-UNSUPPORTED-LUN | IPMI §5.2, Table 5-2, §5.3 | IPMI 68-72 | PASS | Probe sensor LUNs and require an error for advertised unimplemented LUNs, echoing RsLUN |
| CMD-ADDRESS-INVALID-KEY | §10.1.3.3; IPMI §5.3 | 186-187; IPMI 71-72 | PASS | Reject a complete address request with unsupported address-key type |
| CMD-ADDRESS-INCOMPLETE-KEY | §10.1.3.3; IPMI §5.3 | 186-187; IPMI 71-72 | PASS | Reject physical address-key requests lacking the key or site type |
| CMD-GROUP-ID | §10.1.2; IPMI Table 5-1, §5.3.1 | 177; IPMI 68; 71 | PASS | Reserved defining-body identifier does not dispatch a successful VSO command |
| CMD-ADDRESS-RANGES | Rules 4.1.4.1.1-6/-7; Tables 4.1.4.1.1-1/-2 | 38-39 | PASS | Assigned hardware address avoids reserved values and belongs to a defined address class |
| SDR-INFO | Rules 6.2-3/-5; IPMI Table 35-2 | 123; IPMI 481 | PASS | Get Device SDR Info default, sensor count, SDR count, LUN and population fields |
| SDR-RESERVE | Rule 6.2-5; IPMI Table 35-4 | 123; IPMI 483 | PASS | Reserve Device SDR Repository returns a nonzero little-endian two-byte ID |
| SDR-CHAIN | Rules 6.2-3/-8; IPMI §33.12, Table 35-3 | 123-124; IPMI 471; 483 | PASS | Partial-read all record bytes; validate lengths, actual record IDs, noncyclic next chain to FFFFh |
| SDR-PARTIAL-CONSISTENCY | IPMI §35.3, Table 35-3 | IPMI 482-483 | PASS | Compare overlapping reads and reservation-free offset-zero reads against reconstructed records |
| SDR-INVALID-RESERVATION | IPMI §33.11.1, Table 35-3 | IPMI 470; 483 | PASS | Nonzero-offset partial read rejects zero/invalid reservation |
| SDR-CANCEL-RESERVATION | IPMI §33.11.2, §35.4 | IPMI 470; 483 | PASS | A new reservation cancels the preceding reservation for partial reads |
| SDR-BAD-ID | IPMI Table 35-3; Table 5-2, §5.3 | IPMI 483; 70-72 | PASS | Missing actual SDR ID rejected; do not confuse 0000h/FFFFh selectors with missing IDs |
| SDR-BAD-OFFSET | IPMI Table 35-3; Table 5-2, §5.3 | IPMI 483; 70-72 | PASS | Out-of-range SDR offset rejected under a valid reservation |
| SDR-MC-LOCATOR | Rule 6.2-7; IPMI Table 43-8 | 123; IPMI 573-574 | PASS | MC Device Locator record address, sensor/FRU/device-SDR capability bits and string length |
| SDR-SENSOR-RECORDS | Rules 6.2-8/-11; IPMI Tables 43-1/43-2 | 124; IPMI 551-563 | PASS | Full/compact sensor records: fixed lengths, string bounds, key uniqueness and per-LUN readable sensors |
| SENSOR-STATE | Rule 6.2-1; Table 10.2-1; Table 10.2.1-1 | 123; 236; 238 | PASS | Sensor 0 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-LINK | Rule 6.2-1; Table 10.2-1; Table 10.2.2-1 | 123; 236; 241-242 | PASS | Sensor 1 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-HEALTH | Rule 6.2-1; Table 10.2-1; Table 10.2.3-1 | 123; 236; 243 | PASS | Sensor 2 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-VOLTAGE | Rule 6.2-1; Table 10.2-1; Table 10.2.4-1 | 123; 236; 245 | PASS | Sensor 3 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-TEMPERATURE | Rule 6.2-1; Table 10.2-1; Table 10.2.5-1 | 123; 236; 246-247 | PASS | Sensor 4 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-TEST-RESULTS | Rule 6.2-1; Table 10.2-1; Table 10.2.6-1 | 123; 236; 248 | PASS | Sensor 5 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-TEST-STATUS | Rule 6.2-1; Table 10.2-1; Table 10.2.7-1 | 123; 236; 249 | PASS | Sensor 6 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-UNASSIGNED | IPMI Table 35-15; Table 5-2, §5.3 | IPMI 497; 70-72 | PASS | Unknown sensor number yields CBh or the allowed generic unspecified error |
| SENSOR-TYPE-RESERVATIONS | Rule 6.2-12; Table 10.2-3 | 124; 237 | PASS | No sensor uses F9h..FFh reserved sensor types |
| FRU-AREA-INFO | Rules 8.1-1/-5; IPMI Table 34-2 | 138-139; IPMI 476 | PASS | FRU 0 on LUN 0: exact inventory info layout and byte/word access encoding |
| FRU-READ | Rule 8.1-5; IPMI Table 34-3 | 139; IPMI 477 | PASS | Read entire inventory in bounded chunks; validate returned counts and byte-exact overlapping reads |
| FRU-READ-OFFSET | IPMI Table 34-3; Table 5-2, §5.3 | IPMI 477; 70-72 | PASS | Read starting past the FRU size rejected with out-of-range or permitted unspecified error |
| FRU-READ-COUNT | IPMI Table 34-3; Table 5-2, §5.3 | IPMI 477; 70-72 | PASS | Reject zero read count; oversized reads return bounded data or a permitted transfer-size error |
| FRU-COMMON-HEADER | Rule 8.1-2; FRU Table 8-1 | 138; FRU 5 | PASS | Common header version, offsets, mandatory Board/Product areas, pad and checksum |
| FRU-NO-CHASSIS-AREA | Rule 8.1-3 | 139 | PASS | Board FRU common header has zero Chassis Info Area offset |
| FRU-BOARD-AREA | Rule 8.1-2; FRU Table 11-1 | 138; FRU 7 | PASS | BOARD area version, size, checksum, required type/length fields, terminator and padding |
| FRU-PRODUCT-AREA | Rule 8.1-2; FRU Table 12-1 | 138; FRU 8 | PASS | PRODUCT area version, size, checksum, required type/length fields, terminator and padding |
| FRU-MULTIRECORD | FRU §§16-17; VITA Table 10.3-1 | FRU 13-15; 255 | PASS | If present, validate MultiRecord header/data checksums, length and end-of-list |
| FRU-WRITE | Rule 8.1-5; IPMI Table 34-4; Rules 8-4/-5 | 139; IPMI 478; 137 | PASS | Write original header bytes, validate count or 80h protection, restore original bytes and verify full image |
| FRU-WRITE-OFFSET | IPMI Table 34-4; Table 5-2, §5.3 | IPMI 478; 70-72 | PASS | Reject out-of-range write; original full image restored and verified even after failure |
| LINK-A-ACCESS | Rules 4.1.1-1/-5; 9.2.1.1-1 | 26; 28; 160 | PASS | Implemented mandatory IPMB-A accepts a valid request |
| LINK-STATE-NO-CHANGE | §10.1.3.7; Rule 9.3-10 | 189-190; 170 | PASS | Set IPMB State FFh no-change leaves override and speed fields unchanged |
| LINK-REFUSE-ISOLATION | Rule 4.1.8-3 | 68 | PASS | Refuse isolation of both interfaces and remain reachable without changing overrides |
| LINK-UNSUPPORTED-ATOMIC | Rule 4.1.1-14; §10.1.3.7 | 31; 189-190 | PASS | Unsupported link/speed parameters return CCh and make no change |
| LINK-OPTIONAL-SPEED | Table 3.3-1; §10.1.3.7 | 24-25; 189-190 | PASS | 2022 controller accepts the optional speed byte at current supported speed |
| TIMING-T5 | Rule 4.1.1-11; Table 4.1.1-1 T5 | 30-31 | PASS | 227 ms STOP-to-STOP deadline per request, with no adapter retries masking slow responses |
| BROADCAST-ADDRESSED | Table 10.1.1-1; IPMI §20.9, Fig.20-1 | 172; IPMI 280 | PASS | General-call prefix plus embedded own address gives exactly one correctly sourced Device ID response |
| BROADCAST-OTHER-ADDRESS | IPMI §20.9, Fig.20-1 | IPMI 280 | PASS | Controller ignores a broadcast with an embedded address other than its own |
| FRAME-BAD-HEADER-CHECKSUM | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore bad header checksum frame and remain responsive |
| FRAME-BAD-DATA-CHECKSUM | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore bad data checksum frame and remain responsive |
| FRAME-SHORT | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore short frame and remain responsive |
| FRAME-WRONG-DESTINATION | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore wrong destination frame and remain responsive |
| DUPLICATE-READ | IPMI Table 5-2, §5.3; Obs.4.1.1-6 | IPMI 70-72; 30 | PASS | Identical Device ID request and sequence receive an equivalent response or legal duplicate error |
| DUPLICATE-WRITE-REPLY | IPMI Table 5-2; Table 34-4; Obs.4.1.1-6 | IPMI 70; 478; 30 | PASS | Repeat an identical checksum-preserving FRU write with the same sequence; restore and verify image |
| EVENT-NO-GENERATION | Rule 7-4 | 134 | PASS | Observe a bounded quiet capture window and reject any IPMC Platform Event request |

PASS: 98; FAIL: 0
