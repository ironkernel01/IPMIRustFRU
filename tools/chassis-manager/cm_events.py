"""cm_events.py - the Chassis Manager as Event Receiver (VITA 46.11 Rules 7-3 and 7-5).

A Tier-2 or Tier-3 IPMC sends Platform Event requests (NetFn 04h, command 02h) to the Event
Receiver, the Chassis Manager at 20h. `EventReceiver.handle` takes one request frame addressed to
the Chassis Manager and returns the response frame (or None when the run withholds the answer),
and records every request with its decoded fields. `Link` (cm_link.py) calls it for each request
that arrives, while it waits for a response and between requests.

 - Tier 2 answers Platform Event with 00h; Tier 1 is not an Event Receiver (Rule 7-2) and answers
   C1h. Any other request to the Chassis Manager is answered C1h.
 - The first `withhold` Platform Event frames (retries included) get no answer.
 - A repeat of a request (same rqSA, sequence number, command and data within T9) is answered
   again and recorded as a duplicate, not as a new event.
"""

from cm_link import (
    LUN_MASK, SEQ_SHIFT, T9_S, frame_payload, response_to,
)

NETFN_SENSOR = 0x04
CMD_PLATFORM_EVENT = 0x02
CC_OK = 0x00
CC_INVALID_COMMAND = 0xC1

# Platform Event request data (IPMI v2.0 Table 29-3): EvMRev, sensor type, sensor number, event
# direction (bit 7: 1 = deassertion) and event type (bits 6:0), then event data 1..3.
EVENT_DIRECTION_BIT = 0x80
EVENT_TYPE_MASK = 0x7F
EVENT_DATA_OFFSET_MASK = 0x0F  # ED1 bits 3:0 (and ED2 bits 3:0): the event offset / a state
EVENT_CAUSE_SHIFT = 4  # ED2 bits 7:4: the cause of a FRU State change
EVENT_MIN_DATA = 3  # EvMRev, sensor type, sensor number: enough to name a sensor

# Sensor types (IPMI v2.0 Table 42-3 and VITA 46.11 section 10.2).
SENSOR_TYPE_VOLTAGE = 0x02
SENSOR_TYPE_FRU_STATE = 0xF0
SENSOR_TYPE_IPMB_LINK = 0xF1
SENSOR_TYPE_FRU_HEALTH = 0xF2
SENSOR_TYPE_FRU_TEMPERATURE = 0xF3
SENSOR_TYPE_PAYLOAD_TEST_RESULTS = 0xF4
SENSOR_TYPE_PAYLOAD_TEST_STATUS = 0xF5
SENSOR_TYPE_PAYLOAD_MODE = 0xF7
SENSOR_TYPE_IPMC_RESET_TYPE = 0xF8

# Table 10.2.1-3: the cause of a FRU State change (ED2 [7:4]).
FRU_STATE_CAUSES = {
    0x0: "normal state change",
    0x1: "change commanded by the Chassis Manager",
    0x3: "FRU programmatic action",
    0x6: "surprise state change due to power failure",
    0x8: "invalid hardware address",
    0x9: "unexpected deactivation",
    0xF: "unknown",
}
IPMB_LINK_OFFSETS = {
    0: "A off B off", 1: "A on B off", 2: "A off B on", 3: "A on B on",
    4: "IPMB-A 400 kHz", 5: "IPMB-B 400 kHz",
}
FRU_HEALTH_OFFSETS = {0: "good", 1: "predictive failure"}
FRU_VOLTAGE_OFFSETS = {0: "limit not exceeded", 1: "limit exceeded"}
FRU_TEMPERATURE_OFFSETS = (
    "lower non-critical", "lower critical", "lower non-recoverable",
    "upper non-critical", "upper critical", "upper non-recoverable",
)
# Sensors whose offset is shown with its number only.
PLAIN_SENSORS = {
    SENSOR_TYPE_PAYLOAD_TEST_RESULTS: "Payload Test Results",
    SENSOR_TYPE_PAYLOAD_TEST_STATUS: "Payload Test Status",
    SENSOR_TYPE_PAYLOAD_MODE: "Payload Mode",
    SENSOR_TYPE_IPMC_RESET_TYPE: "IPMC Reset Type",
}


def event_text(sensor_type: int, deassert: bool, data: list) -> str:
    """Text of one event for the log, from the sensor type, the direction and the event data
    bytes (ED1..ED3 as far as present)."""
    ed1 = data[0] if data else None
    offset = None if ed1 is None else ed1 & EVENT_DATA_OFFSET_MASK
    if offset is None:
        return f"sensor type {sensor_type:02X}h without event data"
    if sensor_type == SENSOR_TYPE_FRU_STATE:
        if len(data) < 2:
            return f"FRU State M{offset}"
        cause = data[1] >> EVENT_CAUSE_SHIFT
        return (f"FRU State M{offset} from M{data[1] & EVENT_DATA_OFFSET_MASK}, cause: "
                f"{FRU_STATE_CAUSES.get(cause, f'cause {cause:X}h')}")
    if sensor_type == SENSOR_TYPE_IPMB_LINK:
        return f"System IPMB Link {IPMB_LINK_OFFSETS.get(offset, f'offset {offset:X}h')}"
    if sensor_type == SENSOR_TYPE_FRU_HEALTH:
        return f"FRU Health {FRU_HEALTH_OFFSETS.get(offset, f'offset {offset:X}h')}"
    if sensor_type == SENSOR_TYPE_VOLTAGE:
        return f"FRU Voltage {FRU_VOLTAGE_OFFSETS.get(offset, f'offset {offset:X}h')}"
    if sensor_type == SENSOR_TYPE_FRU_TEMPERATURE:
        what = (FRU_TEMPERATURE_OFFSETS[offset] if offset < len(FRU_TEMPERATURE_OFFSETS)
                else f"offset {offset:X}h")
        return f"FRU Temperature {what} {'deasserted' if deassert else 'asserted'}"
    if sensor_type in PLAIN_SENSORS:
        return f"{PLAIN_SENSORS[sensor_type]} offset {offset:X}h"
    return f"sensor type {sensor_type:02X}h data " + " ".join(f"{b:02X}" for b in data)


def decode_event(data: bytes) -> dict:
    """The fields of a Platform Event request's data (None where the data is short) and their
    text."""
    fields = (list(data) + [None] * 7)[:7]
    rev, stype, number, dirtype, ed1, ed2, ed3 = fields
    deassert = bool(dirtype is not None and dirtype & EVENT_DIRECTION_BIT)
    present = [b for b in (ed1, ed2, ed3) if b is not None]
    if len(data) < EVENT_MIN_DATA:
        text = f"short event ({len(data)} bytes)"
    else:
        text = event_text(stype, deassert, present)
    return {
        "ev_m_rev": rev, "sensor_type": stype, "sensor_number": number,
        "direction": None if dirtype is None else "deassert" if deassert else "assert",
        "event_type": None if dirtype is None else dirtype & EVENT_TYPE_MASK,
        "ed1": ed1, "ed2": ed2, "ed3": ed3, "text": text,
    }


class EventReceiver:
    """Answers and records the requests addressed to the Chassis Manager."""

    def __init__(self, now_s, tier: int = 2, withhold: int = 0, log=None, clock=None) -> None:
        """`now_s()` is the run time in seconds (for the record), `clock()` a monotonic clock for
        T9 (default: `now_s`), `log(text)` receives one line per request."""
        if tier not in (1, 2):
            raise ValueError("the Chassis Manager tier is 1 or 2")
        if withhold < 0:
            raise ValueError("withhold must be 0 or more")
        self.now_s, self.tier, self.log = now_s, tier, log
        self._clock = clock or now_s
        self._withhold_left = withhold
        self.events: list = []  # every Platform Event frame received, in arrival order
        self.other_requests: list = []  # requests to the Chassis Manager that are not events
        # (rqSA, rqLUN, NetFn, cmd, rqSeq) -> (clock() of the last frame, its data): the
        # retransmission key of IPMI v2.0 16.3, which does not include the data
        self._seen: dict = {}
        self.events_received = 0  # distinct events (a repeat is not counted again)
        self.events_duplicates = 0  # includes the sequence reuses below
        self.events_seq_reuse = 0  # same key inside T9 with other event data
        self.events_withheld = 0
        self.events_refused = 0

    def counts(self) -> dict:
        """The totals for the JSON summary."""
        return {"events_received": self.events_received,
                "events_duplicates": self.events_duplicates,
                "events_seq_reuse": self.events_seq_reuse,
                "events_withheld": self.events_withheld,
                "events_refused": self.events_refused}

    def handle(self, bus: str, frame: bytes):
        """Process one request frame; the response frame, or None when the answer is withheld."""
        rq_sa, netfn = frame[3], frame[1] >> 2
        seq, cmd = frame[4] >> SEQ_SHIFT, frame[5]
        data = frame_payload(frame)
        record = {"t_s": round(self.now_s(), 3), "bus": bus, "rq_sa": f"0x{rq_sa:02X}",
                  "seq": seq, "netfn": netfn, "cmd": cmd, "rq_lun": frame[4] & LUN_MASK}
        if (netfn, cmd) != (NETFN_SENSOR, CMD_PLATFORM_EVENT):
            record.update(answered_cc=CC_INVALID_COMMAND, data=data.hex(" "))
            self.other_requests.append(record)
            self._say(f"request NetFn {netfn:02X}h cmd {cmd:02X}h from {rq_sa:02X}h on bus {bus} "
                      f"seq {seq:02X}h: answered {CC_INVALID_COMMAND:02X}h")
            return response_to(frame, bytes([CC_INVALID_COMMAND]))
        record.update(decode_event(data))
        key = (rq_sa, record["rq_lun"], netfn, cmd, seq)
        now = self._clock()
        last = self._seen.get(key)
        record["duplicate"] = last is not None and now - last[0] <= T9_S
        # A real Event Receiver drops the event of a repeated key, so a reuse with other data
        # loses an event: it is a defect of the requester, counted and said apart.
        record["seq_reuse"] = record["duplicate"] and last[1] != bytes(data)
        self._seen[key] = (now, bytes(data))
        if record["seq_reuse"]:
            self.events_seq_reuse += 1
            self._say(f"sequence reuse: seq {seq:02X}h of {rq_sa:02X}h came again inside T9 "
                      f"with other event data ({last[1].hex(' ')} then {bytes(data).hex(' ')}); "
                      f"treated as a duplicate")
        if record["duplicate"]:
            self.events_duplicates += 1
        else:
            self.events_received += 1
        if self._withhold_left > 0:
            self._withhold_left -= 1
            self.events_withheld += 1
            record.update(withheld=True, answered_cc=None)
            answer = None
        else:
            cc = CC_OK if self.tier == 2 else CC_INVALID_COMMAND
            if cc != CC_OK:
                self.events_refused += 1
            record.update(withheld=False, answered_cc=cc)
            answer = response_to(frame, bytes([cc]))
        self.events.append(record)
        self._say_event(record)
        return answer

    def _say_event(self, r: dict) -> None:
        status = ("withheld" if r["withheld"] else f"answered {r['answered_cc']:02X}h"
                  + (" (refused: Tier-1 Chassis Manager)" if r["answered_cc"] else ""))
        self._say(f"event from {r['rq_sa'][2:]}h on bus {r['bus']} seq {r['seq']:02X}h "
                  f"EvMRev {_hex(r['ev_m_rev'])} type {_hex(r['sensor_type'])} sensor "
                  f"{r['sensor_number']} {r['direction']} event type {_hex(r['event_type'])} "
                  f"ED {_hex(r['ed1'])} {_hex(r['ed2'])} {_hex(r['ed3'])}: {r['text']}"
                  f"{' [duplicate]' if r['duplicate'] else ''}"
                  f"{' [sequence reuse]' if r['seq_reuse'] else ''} -> {status}")

    def _say(self, text: str) -> None:
        if self.log:
            self.log(text)


def _hex(value) -> str:
    return "--" if value is None else f"{value:02X}"
