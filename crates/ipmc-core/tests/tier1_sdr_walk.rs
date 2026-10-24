// tier1_sdr_walk.rs - enumerates the Device SDR repository the way a Chassis Manager does, through
// the Link and the dispatcher: Reserve Device SDR Repository, then for each record from 0000h the
// 5-byte header, then the rest in 16-byte chunks under the reservation, following the next
// record IDs to FFFFh (IPMI v2.0 sections 33.11, 33.12, 35.3, 35.4). Every record is reassembled
// from the wire and compared with the repository's own bytes.

mod common;
use common::{Rig, SENSOR_COUNT};
use ipmc_core::ipmb::NETFN_SE;
use ipmc_core::queue::Bus;

const GET_DEVICE_SDR: u8 = 0x21;
const RESERVE_DEVICE_SDR: u8 = 0x22;
const CC_OK: u8 = 0x00;

/// SDR record header (IPMI v2.0 Table 43-2 "RECORD HEADER"): record ID LS and MS, SDR version,
/// record type, record length (bytes after the header).
const HEADER_LEN: usize = 5;
const HEADER_TYPE: usize = 3;
const HEADER_LENGTH: usize = 4;
/// Record type 02h compact sensor record (Table 43-2) and 12h management controller device
/// locator record (Table 43-8); the sensor number is byte 8 of a compact record (index 7).
const TYPE_COMPACT: u8 = 0x02;
const TYPE_MC_LOCATOR: u8 = 0x12;
const SENSOR_NUMBER_INDEX: usize = 7;
/// The Chassis Manager of the capture reads 16 bytes per request.
const CHUNK: usize = 16;
/// NO_NEXT_RECORD -- next record ID FFFFh after the last record (Table 35-3).
const NO_NEXT_RECORD: u16 = 0xFFFF;
/// The most records the walk follows: more than the 9 this IPMC has at the most, so a chain that
/// never reaches FFFFh fails the test instead of hanging it.
const WALK_LIMIT: usize = 16;

/// One Get Device SDR round trip: the next record ID and the bytes returned.
fn read_chunk(
    rig: &mut Rig,
    bus: Bus,
    reservation: [u8; 2],
    id: u16,
    offset: usize,
    count: usize,
) -> (u16, Vec<u8>) {
    let [id_lo, id_hi] = id.to_le_bytes();
    let request = [
        reservation[0],
        reservation[1],
        id_lo,
        id_hi,
        u8::try_from(offset).unwrap(),
        u8::try_from(count).unwrap(),
    ];
    let answer = rig.send(bus, NETFN_SE, GET_DEVICE_SDR, &request);
    assert_eq!(answer.cc, CC_OK, "record {id:04X}h offset {offset}");
    let (next, bytes) = answer.data.split_at(2);
    assert_eq!(bytes.len(), count, "record {id:04X}h offset {offset}");
    (u16::from_le_bytes([next[0], next[1]]), bytes.to_vec())
}

#[test]
fn a_chassis_manager_walk_reads_every_record_back() {
    let mut rig = Rig::new().both_buses();
    let reserved = rig.send(Bus::A, NETFN_SE, RESERVE_DEVICE_SDR, &[]);
    assert_eq!(reserved.cc, CC_OK);
    let reservation = [reserved.data[0], reserved.data[1]];

    let mut id = 0u16;
    let mut walked: Vec<(u16, Vec<u8>)> = Vec::new();
    for step in 0..WALK_LIMIT {
        // Alternate the buses like the live Chassis Manager; the reservation is link-wide.
        let bus = if step % 2 == 0 { Bus::A } else { Bus::B };
        let (next, header) = read_chunk(&mut rig, bus, reservation, id, 0, HEADER_LEN);
        let total = HEADER_LEN + usize::from(header[HEADER_LENGTH]);
        let mut record = header;
        while record.len() < total {
            let count = CHUNK.min(total - record.len());
            let (chunk_next, chunk) =
                read_chunk(&mut rig, bus, reservation, id, record.len(), count);
            assert_eq!(chunk_next, next, "next ID is the same in every chunk");
            record.extend_from_slice(&chunk);
        }
        walked.push((id, record));
        if next == NO_NEXT_RECORD {
            break;
        }
        assert_ne!(next, id, "the chain moves on");
        id = next;
    }

    // 7 sensor records and the locator; 8 sensor records with the Payload Mode sensor at Tier-3.
    let sensors = usize::from(SENSOR_COUNT);
    assert_eq!(
        walked.len(),
        sensors + 1,
        "the sensor records and the locator"
    );
    for (index, (id, record)) in walked.iter().enumerate() {
        let index = u16::try_from(index).unwrap();
        // The record ID in the header, LS byte first, is the ID the record was reached by.
        assert_eq!(*id, index);
        assert_eq!(u16::from_le_bytes([record[0], record[1]]), index);
        // The length byte counts every byte after the header.
        assert_eq!(
            usize::from(record[HEADER_LENGTH]) + HEADER_LEN,
            record.len()
        );
        if usize::from(index) < sensors {
            assert_eq!(record[HEADER_TYPE], TYPE_COMPACT, "record {index}");
            assert_eq!(
                u16::from(record[SENSOR_NUMBER_INDEX]),
                index,
                "sensors 0.. in order"
            );
        } else {
            assert_eq!(record[HEADER_TYPE], TYPE_MC_LOCATOR, "the locator is last");
        }
        // The bytes read over the wire are the repository's own.
        assert_eq!(
            Some(&record[..]),
            rig.ctx.sdr.record(*id),
            "record {index} reassembled"
        );
    }
}
