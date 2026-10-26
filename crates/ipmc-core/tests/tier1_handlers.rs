// tier1_handlers.rs - every Tier-1 handler added in Phase 4, driven through `dispatch` with
// hand-written expected bytes taken from the IPMI v2.0 rev 1.1 and VITA 46.11-2022 tables (not
// produced by the encoders under test), every completion code of every error path.

mod common;
use common::{by_tier, call, call_vso, locator, sensor0, SENSOR_COUNT};
use ipmc_core::addr;
use ipmc_core::dispatch::{Context, SensorReadError};
use ipmc_core::fru_state::State;
use ipmc_core::ipmb::{NETFN_APP, NETFN_SE, NETFN_STORAGE};
use ipmc_core::link::{BusMode, LinkConfig, LinkView, TxPolicy};
use ipmc_core::sensor::{PowerState, RailStatus, SensorSnapshot, Summary, TEMP_FLAG_UC};
use ipmc_core::trace::NoTrace;
use ipmc_fru as fru;

const OK: u8 = 0x00;
const NVMRO_PROTECTED: u8 = 0x80;
const BAD_CMD_LEN: u8 = 0xC7;
const BAD_RESERVATION: u8 = 0xC5;
const OUT_OF_RANGE: u8 = 0xC9;
const CANT_RETURN: u8 = 0xCA;
const NOT_PRESENT: u8 = 0xCB;
const BAD_FIELD: u8 = 0xCC;
const DEST_UNAVAILABLE: u8 = 0xD3;

// Command codes (IPMI v2.0 Appendix G; VITA 46.11 Table 10.1.2-1).
const GET_DEVICE_SDR_INFO: u8 = 0x20;
const GET_DEVICE_SDR: u8 = 0x21;
const RESERVE_DEVICE_SDR: u8 = 0x22;
const GET_SENSOR_READING: u8 = 0x2D;
const GET_FRU_AREA_INFO: u8 = 0x10;
const READ_FRU_DATA: u8 = 0x11;
const WRITE_FRU_DATA: u8 = 0x12;
const GET_SELF_TEST_RESULTS: u8 = 0x04;
const GET_VSO_CAPABILITIES: u8 = 0x00;
const SET_IPMB_STATE: u8 = 0x09;
const GET_DEVICE_LOCATOR_RECORD_ID: u8 = 0x0D;
const GET_FRU_ADDRESS_INFO: u8 = 0x40;

fn ctx() -> Context {
    Context::new(addr::resolve(0x7F))
}

/// A context whose link view says IPMB-B is fitted (power-up state: A enabled, B isolated).
fn ctx_with_b() -> Context {
    let mut c = ctx();
    c.link = LinkView::power_up(
        LinkConfig {
            b_fitted: true,
            tx_policy: TxPolicy::Alternate,
            requests_allowed: false,
        },
        true,
    );
    c
}

fn storage(c: &mut Context, cmd: u8, data: &[u8]) -> common::Reply {
    call(c, NETFN_STORAGE, cmd, data)
}

fn se(c: &mut Context, cmd: u8, data: &[u8]) -> common::Reply {
    call(c, NETFN_SE, cmd, data)
}

fn expect_err(reply: common::Reply, code: u8) {
    assert_eq!(reply.cc, code, "completion code");
    assert!(reply.data.is_empty(), "an error carries no data");
}

// ---- Get FRU Inventory Area Info (IPMI v2.0 Table 34-2) ----

// ---- commands whose only parameter is the FRU Device ID ----

/// FruIdCommand -- a command that takes the FRU Device ID alone: Get FRU Inventory Area Info
/// (IPMI v2.0 Table 34-2) and Get Device Locator Record ID (VITA 46.11 Table 10.1.3.11-1).
struct FruIdCommand {
    name: &'static str,
    call: fn(&mut Context, u8, &[u8]) -> common::Reply,
    cmd: u8,
    /// The response data for FRU Device ID 0.
    answer: &'static [u8],
}

const FRU_ID_COMMANDS: [FruIdCommand; 2] = [
    FruIdCommand {
        name: "Get FRU Inventory Area Info",
        call: storage,
        cmd: GET_FRU_AREA_INFO,
        // Size 512 = 0200h LS byte first; byte 4 bit 0 = 0, accessed by bytes.
        answer: &[0x00, 0x02, 0x00],
    },
    FruIdCommand {
        name: "Get Device Locator Record ID",
        call: call_vso,
        cmd: GET_DEVICE_LOCATOR_RECORD_ID,
        // The type 12h record follows the sensors: ID 0007h (0008h with sensor 7 at Tier-3), LS
        // byte first.
        answer: &[SENSOR_COUNT, 0x00],
    },
];

#[test]
fn fru_id_commands_answer_fru_zero_and_refuse_everything_else() {
    for c in FRU_ID_COMMANDS {
        let r = (c.call)(&mut ctx(), c.cmd, &[0x00]);
        assert_eq!((r.cc, r.data), (OK, c.answer.to_vec()), "{}", c.name);
        let mut context = ctx();
        // The request is exactly one byte.
        for data in [&[][..], &[0, 0][..]] {
            expect_err((c.call)(&mut context, c.cmd, data), BAD_CMD_LEN);
        }
        // Above the maximum FRU Device ID (0): Rules 4.1.1-17 and 4.2-3; FFh is reserved.
        for id in [0x01u8, 0xFF] {
            expect_err((c.call)(&mut context, c.cmd, &[id]), OUT_OF_RANGE);
        }
    }
}

// ---- Read FRU Data (Table 34-3) ----

/// The common header the image starts with (FRU spec Table 8-1; ipmc-fru records.rs derives the same bytes).
const HEADER: [u8; 8] = [0x01, 0x00, 0x00, 0x01, 0x12, 0x28, 0x00, 0xC4];

/// le -- an offset as the LS and MS bytes of a FRU data request.
const fn le(offset: usize) -> [u8; 2] {
    (offset as u16).to_le_bytes()
}

/// FREE_AT -- an offset in the unused tail of the area, past the built areas (00h bytes).
const FREE_AT: usize = fru::IMAGE_LEN + 8;

#[test]
fn read_fru_data_in_16_byte_chunks_like_the_chassis_manager() {
    let mut c = ctx();
    let mut image = Vec::new();
    for chunk in 0..fru::SIZE / 16 {
        // The capture reads offsets 00h, 10h, 20h ... with count 10h.
        let [lo, hi] = le(chunk * 16);
        let r = storage(&mut c, READ_FRU_DATA, &[0x00, lo, hi, 0x10]);
        assert_eq!(r.cc, OK, "chunk {chunk}");
        assert_eq!(r.data[0], 0x10, "count returned is 1-based");
        assert_eq!(r.data.len(), 17);
        image.extend_from_slice(&r.data[1..]);
    }
    assert_eq!(image.len(), fru::SIZE);
    assert_eq!(&image[..8], &HEADER);
    assert_eq!(image, c.fru.bytes());
    // Areas read back through the decoder: checksums good, strings as configured.
    let head = fru::common_header(&image).unwrap();
    assert!(head.checksum_ok);
    let board = fru::area(&image, fru::AreaKind::Board, head.board).unwrap();
    assert!(board.checksum_ok);
    assert_eq!(board.fields().next().unwrap().data, b"IronKernel");
}

#[test]
fn read_fru_data_longest_chunk_is_23_bytes() {
    // The response data holds the count byte and the data: 24 bytes at most (IPMB 32 - 8).
    let r = storage(&mut ctx(), READ_FRU_DATA, &[0x00, 0x00, 0x00, 23]);
    assert_eq!(r.cc, OK);
    assert_eq!((r.data[0], r.data.len()), (23, 24));
    assert_eq!(&r.data[1..9], &HEADER);
}

// ---- Write FRU Data (Table 34-4) ----

#[test]
fn write_then_read_back_changes_the_ram_store() {
    let mut c = ctx();
    let [lo, hi] = le(FREE_AT);
    let [before_lo, before_hi] = le(FREE_AT - 1);
    let r = storage(&mut c, WRITE_FRU_DATA, &[0x00, lo, hi, 0xAA, 0xBB, 0xCC]);
    assert_eq!((r.cc, r.data), (OK, vec![3]), "count written, 1-based");
    let r = storage(&mut c, READ_FRU_DATA, &[0x00, before_lo, before_hi, 5]);
    assert_eq!((r.cc, r.data), (OK, vec![5, 0x00, 0xAA, 0xBB, 0xCC, 0x00]));
    // The longest write a 32-byte request frame carries: 25 data bytes, 3 of them the FRU ID and
    // the offset, so 22 bytes to write. The offset is in the unused (00h) part of the area.
    let long: Vec<u8> = (1..=22).collect();
    let [long_lo, long_hi] = le(FREE_AT + 8);
    let mut request = vec![0x00, long_lo, long_hi];
    request.extend_from_slice(&long);
    assert_eq!(storage(&mut c, WRITE_FRU_DATA, &request).data, vec![22]);
    let r = storage(&mut c, READ_FRU_DATA, &[0x00, long_lo, long_hi, 22]);
    assert_eq!(&r.data[1..], &long[..]);
    // A write to the last byte of the area.
    let [last_lo, last_hi] = le(fru::SIZE - 1);
    assert_eq!(
        storage(&mut c, WRITE_FRU_DATA, &[0x00, last_lo, last_hi, 0x5A]).data,
        vec![1]
    );
    assert_eq!(c.fru.bytes()[fru::SIZE - 1], 0x5A);
}

/// fru_data_error_paths_refuse_with_their_completion_codes -- Read FRU Data and Write FRU Data
/// refuse a bad length, a FRU Device ID above 0, a range past the area and a read of more than one
/// frame carries, each with its completion code, and a refused request changes nothing.
#[test]
fn fru_data_error_paths_refuse_with_their_completion_codes() {
    let [size_lo, size_hi] = le(fru::SIZE);
    let [near_lo, near_hi] = le(fru::SIZE - 8);
    let [last_lo, last_hi] = le(fru::SIZE - 1);
    // (command, request data, completion code)
    let rows: Vec<(u8, Vec<u8>, u8)> = vec![
        // Read, length: 4 data bytes exactly.
        (READ_FRU_DATA, vec![], BAD_CMD_LEN),
        (READ_FRU_DATA, vec![0, 0, 0], BAD_CMD_LEN),
        (READ_FRU_DATA, vec![0, 0, 0, 1, 1], BAD_CMD_LEN),
        // Read, FRU Device ID above 0, and count 0 (counts are 1-based).
        (READ_FRU_DATA, vec![1, 0, 0, 8], OUT_OF_RANGE),
        (READ_FRU_DATA, vec![0, 0, 0, 0], OUT_OF_RANGE),
        // Read, offset or offset + count beyond the area.
        (READ_FRU_DATA, vec![0, size_lo, size_hi, 1], OUT_OF_RANGE),
        (READ_FRU_DATA, vec![0, near_lo, near_hi, 16], OUT_OF_RANGE),
        (READ_FRU_DATA, vec![0, 0xFF, 0xFF, 1], OUT_OF_RANGE),
        // Read, inside the area but more than one response frame carries: 24 bytes plus the
        // count byte.
        (READ_FRU_DATA, vec![0, 0, 0, 24], CANT_RETURN),
        (READ_FRU_DATA, vec![0, 0, 0, 0xFF], CANT_RETURN),
        // Write, length: at least one data byte after the three header bytes.
        (WRITE_FRU_DATA, vec![], BAD_CMD_LEN),
        (WRITE_FRU_DATA, vec![0], BAD_CMD_LEN),
        (WRITE_FRU_DATA, vec![0, 0], BAD_CMD_LEN),
        (WRITE_FRU_DATA, vec![0, 0, 0], BAD_CMD_LEN),
        // Write, FRU Device ID above 0, and beyond the area, also across its end.
        (WRITE_FRU_DATA, vec![1, 0, 0, 7], OUT_OF_RANGE),
        (WRITE_FRU_DATA, vec![0, size_lo, size_hi, 7], OUT_OF_RANGE),
        (
            WRITE_FRU_DATA,
            vec![0, last_lo, last_hi, 7, 8],
            OUT_OF_RANGE,
        ),
    ];
    let mut c = ctx();
    let before = c.fru.bytes().to_vec();
    for (cmd, request, code) in rows {
        expect_err(storage(&mut c, cmd, &request), code);
    }
    assert_eq!(
        c.fru.bytes(),
        &before[..],
        "a refused write changes nothing"
    );
    // The last byte of the area is readable.
    let r = storage(&mut c, READ_FRU_DATA, &[0, last_lo, last_hi, 1]);
    assert_eq!((r.cc, r.data), (OK, vec![1, 0]));
}

#[test]
fn only_an_accepted_write_fru_data_marks_the_store_dirty() {
    let mut c = ctx();
    assert!(!c.fru.is_dirty());
    // Refused writes (length, ID, range) leave it clean.
    let [size_lo, size_hi] = le(fru::SIZE);
    for refused in [&[0, 0, 0][..], &[1, 0, 0, 7], &[0, size_lo, size_hi, 7]] {
        storage(&mut c, WRITE_FRU_DATA, refused);
        assert!(!c.fru.is_dirty(), "request {refused:?}");
    }
    // Reads never mark it.
    storage(&mut c, READ_FRU_DATA, &[0, 8, 0, 1]);
    assert!(!c.fru.is_dirty());
    assert_eq!(storage(&mut c, WRITE_FRU_DATA, &[0, 0x08, 0, 0xFF]).cc, OK);
    assert!(c.fru.is_dirty());
    // `set_fru` takes a new image and reruns the self test on it.
    c.set_fru(fru::FruStore::new());
    assert!(!c.fru.is_dirty());
    assert!(c.self_test.passed());
}

#[test]
fn write_fru_data_is_refused_with_80h_while_nvmro_is_asserted() {
    // VITA Rules 8-3 and 8-5: write protect everything, answer 80h, change nothing.
    let mut c = ctx();
    c.addr.nvmro_asserted = true;
    let before = c.fru.bytes().to_vec();
    expect_err(
        storage(&mut c, WRITE_FRU_DATA, &[0x00, 0x08, 0x00, 0xFF]),
        NVMRO_PROTECTED,
    );
    assert_eq!(c.fru.bytes(), &before[..]);
    // Reads stay possible (Rule 8-1), and the write works again once NVMRO clears.
    assert_eq!(storage(&mut c, READ_FRU_DATA, &[0, 8, 0, 1]).cc, OK);
    c.addr.nvmro_asserted = false;
    assert_eq!(storage(&mut c, WRITE_FRU_DATA, &[0, 0x08, 0, 0xFF]).cc, OK);
}

// ---- Get Self Test Results (IPMI v2.0 section 20.4, Table 20-5) ----

fn self_test(c: &mut Context) -> Vec<u8> {
    let r = call(c, NETFN_APP, GET_SELF_TEST_RESULTS, &[]);
    assert_eq!(r.cc, OK);
    r.data
}

/// Board area byte 20 (offset 08h + 14h) is a character of the manufacturer string: changing it
/// breaks the area's zero checksum without touching any length or marker.
const BOARD_STRING_BYTE: u16 = 0x08 + 20;

#[test]
fn a_fresh_context_passes_the_self_test() {
    // 55h: no error, all self tests passed; 00h detail.
    let mut c = ctx();
    assert_eq!(self_test(&mut c), [0x55, 0x00]);
    expect_err(
        call(&mut c, NETFN_APP, GET_SELF_TEST_RESULTS, &[0]),
        BAD_CMD_LEN,
    );
}

#[test]
fn a_write_that_corrupts_the_fru_image_is_recorded_and_undone_by_writing_it_back() {
    let mut c = ctx();
    let offset = BOARD_STRING_BYTE.to_le_bytes();
    let original = c.fru.bytes()[usize::from(BOARD_STRING_BYTE)];
    let write = |c: &mut Context, value: u8| {
        storage(c, WRITE_FRU_DATA, &[0x00, offset[0], offset[1], value])
    };
    // 57h: corrupted data; detail bit 5: cannot access the BMC FRU device (Table 20-5).
    assert_eq!(write(&mut c, original ^ 0xFF).cc, OK);
    assert_eq!(self_test(&mut c), [0x57, 0x20]);
    // Writing the original byte back restores a valid image: 55h 00h.
    assert_eq!(write(&mut c, original).cc, OK);
    assert_eq!(self_test(&mut c), [0x55, 0x00]);
}

#[test]
fn a_failed_self_test_keeps_the_fru_in_m0_until_the_image_is_valid_again() {
    let mut c = ctx();
    let offset = BOARD_STRING_BYTE.to_le_bytes();
    let original = c.fru.bytes()[usize::from(BOARD_STRING_BYTE)];
    storage(
        &mut c,
        WRITE_FRU_DATA,
        &[0x00, offset[0], offset[1], original ^ 0x01],
    );
    assert_eq!(self_test(&mut c), [0x57, 0x20]);
    // Payload power is valid and every other initial check holds, yet the machine stays in M0.
    let on = snapshot(Summary::Good, Summary::Good, 0, PowerState::On);
    assert!(c.update_sensors(1000, &on, &mut NoTrace).is_none());
    assert_eq!(c.fru_state.state(), State::M0);
    storage(
        &mut c,
        WRITE_FRU_DATA,
        &[0x00, offset[0], offset[1], original],
    );
    assert!(c.update_sensors(2000, &on, &mut NoTrace).is_some());
    assert_eq!(c.fru_state.state(), State::M1);
    // A later failure does not take the FRU out of M1 (the result gates only M0 to M1).
    storage(
        &mut c,
        WRITE_FRU_DATA,
        &[0x00, offset[0], offset[1], original ^ 0x01],
    );
    assert_eq!(self_test(&mut c), [0x57, 0x20]);
    assert!(c.update_sensors(3000, &on, &mut NoTrace).is_some());
    assert_eq!(c.fru_state.state(), State::M4);
}

#[test]
fn a_corrupt_persisted_image_loaded_through_set_fru_fails_the_self_test_and_keeps_m0() {
    // The flash log hands `set_fru` the stored image, which can be damaged since it was saved.
    let mut c = ctx();
    let mut stored = *fru::FruStore::new().image();
    stored[usize::from(BOARD_STRING_BYTE)] ^= 0xFF;
    c.set_fru(fru::FruStore::from_image(stored));
    assert_eq!(self_test(&mut c), [0x57, 0x20]);
    let on = snapshot(Summary::Good, Summary::Good, 0, PowerState::On);
    assert!(c.update_sensors(1000, &on, &mut NoTrace).is_none());
    assert_eq!(c.fru_state.state(), State::M0);
    // Writing the original byte back repairs it: 55h 00h, and the FRU leaves M0.
    let offset = BOARD_STRING_BYTE.to_le_bytes();
    let original = fru::FruStore::new().image()[usize::from(BOARD_STRING_BYTE)];
    storage(
        &mut c,
        WRITE_FRU_DATA,
        &[0x00, offset[0], offset[1], original],
    );
    assert_eq!(self_test(&mut c), [0x55, 0x00]);
    assert!(c.update_sensors(2000, &on, &mut NoTrace).is_some());
    assert_eq!(c.fru_state.state(), State::M1);
}

// ---- Get Device SDR Info (Table 35-2) ----

#[test]
fn sdr_info_counts_sensors_and_records() {
    let mut c = ctx();
    // No operation byte, or operation 0b: the sensor count (7, with the Payload Mode sensor 8 at
    // Tier-3), flags 01h (LUN 0 has sensors, static).
    for request in [&[][..], &[0x00][..]] {
        let r = se(&mut c, GET_DEVICE_SDR_INFO, request);
        assert_eq!((r.cc, r.data), (OK, vec![SENSOR_COUNT, 0x01]));
    }
    // Operation 1b: SDR count, the sensor records and the locator record.
    let r = se(&mut c, GET_DEVICE_SDR_INFO, &[0x01]);
    assert_eq!((r.cc, r.data), (OK, vec![SENSOR_COUNT + 1, 0x01]));
    expect_err(se(&mut c, GET_DEVICE_SDR_INFO, &[0x02]), BAD_FIELD);
    expect_err(se(&mut c, GET_DEVICE_SDR_INFO, &[0x80]), BAD_FIELD);
    expect_err(se(&mut c, GET_DEVICE_SDR_INFO, &[0, 0]), BAD_CMD_LEN);
}

// ---- Reserve Device SDR Repository (Table 35-4) and Get Device SDR (Table 35-3) ----

/// scanning -- the status byte of an available sensor: scanning enabled (bit 6) and, at
/// Tier-2/3, event messages enabled (bit 7; IPMI v2.0 Table 35-14).
fn scanning() -> u8 {
    by_tier([0x40, 0xC0, 0xC0])
}

/// unavailable -- `scanning` with the reading unavailable (bit 5).
fn unavailable() -> u8 {
    by_tier([0x60, 0xE0, 0xE0])
}

fn reserve(c: &mut Context) -> [u8; 2] {
    let r = se(c, RESERVE_DEVICE_SDR, &[]);
    assert_eq!(r.cc, OK);
    r.data.try_into().expect("two reservation bytes")
}

/// Get Device SDR request: reservation, record ID, offset, count.
fn sdr_request(reservation: [u8; 2], id: u16, offset: u8, count: u8) -> [u8; 6] {
    let [id_lo, id_hi] = id.to_le_bytes();
    [reservation[0], reservation[1], id_lo, id_hi, offset, count]
}

#[test]
fn reserve_returns_incrementing_non_zero_ids() {
    let mut c = ctx();
    assert_eq!(reserve(&mut c), [0x01, 0x00]);
    assert_eq!(reserve(&mut c), [0x02, 0x00]);
    expect_err(se(&mut c, RESERVE_DEVICE_SDR, &[0]), BAD_CMD_LEN);
}

#[test]
fn a_record_reads_back_in_chunks_with_the_reservation() {
    let mut c = ctx();
    let res = reserve(&mut c);
    // The Chassis Manager of the capture reads 16 bytes at offsets 0 and 16, then the rest.
    let mut record = Vec::new();
    for (offset, count) in [(0u8, 16u8), (16, 16), (32, 9)] {
        let r = se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, offset, count));
        assert_eq!(r.cc, OK, "offset {offset}");
        assert_eq!(&r.data[..2], &[0x01, 0x00], "next record ID is 0001h");
        record.extend_from_slice(&r.data[2..]);
    }
    assert_eq!(record, sensor0());
}

#[test]
fn partial_reads_need_the_current_reservation() {
    let mut c = ctx();
    let first = reserve(&mut c);
    let second = reserve(&mut c);
    // Offset 0 ignores the reservation field (Table 35-3: "use 0000h otherwise").
    assert_eq!(
        se(&mut c, GET_DEVICE_SDR, &sdr_request([0, 0], 0, 0, 16)).cc,
        OK
    );
    // A non-zero offset with the stale, a never-issued and the zero reservation: C5h.
    for stale in [first, [0x09, 0x00], [0x00, 0x00]] {
        expect_err(
            se(&mut c, GET_DEVICE_SDR, &sdr_request(stale, 0, 16, 16)),
            BAD_RESERVATION,
        );
    }
    assert_eq!(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(second, 0, 16, 16)).cc,
        OK
    );
}

#[test]
fn whole_record_read_and_the_next_id_chain_ends_at_ffff() {
    let mut c = ctx();
    // The locator record is the last one: FFh reads all 20 bytes in one response, next ID FFFFh.
    let last = u16::from(SENSOR_COUNT);
    let r = se(&mut c, GET_DEVICE_SDR, &sdr_request([0, 0], last, 0, 0xFF));
    assert_eq!(r.cc, OK);
    assert_eq!(&r.data[..2], &[0xFF, 0xFF]);
    assert_eq!(&r.data[2..], &locator());
    // Record ID FFFFh names the last record too.
    let r = se(
        &mut c,
        GET_DEVICE_SDR,
        &sdr_request([0, 0], 0xFFFF, 0, 0xFF),
    );
    assert_eq!(&r.data[2..], &locator());
    // The chain 0000h -> 0001h -> ... -> the locator's ID -> FFFFh, from 5-byte header reads.
    let mut id = 0u16;
    let mut ids = vec![id];
    loop {
        let r = se(&mut c, GET_DEVICE_SDR, &sdr_request([0, 0], id, 0, 5));
        assert_eq!(r.cc, OK);
        assert_eq!(
            u16::from_le_bytes([r.data[2], r.data[3]]),
            id,
            "header record ID"
        );
        let next = u16::from_le_bytes([r.data[0], r.data[1]]);
        if next == 0xFFFF {
            break;
        }
        ids.push(next);
        id = next;
    }
    assert_eq!(ids, (0..=last).collect::<Vec<u16>>());
}

#[test]
fn sdr_error_paths() {
    let mut c = ctx();
    let res = reserve(&mut c);
    for bad in [&[][..], &[0; 5][..], &[0; 7][..]] {
        expect_err(se(&mut c, GET_DEVICE_SDR, bad), BAD_CMD_LEN);
    }
    // No such record.
    expect_err(
        se(
            &mut c,
            GET_DEVICE_SDR,
            &sdr_request(res, u16::from(SENSOR_COUNT) + 1, 0, 5),
        ),
        NOT_PRESENT,
    );
    expect_err(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0x0100, 0, 5)),
        NOT_PRESENT,
    );
    // Offset past the record, or the count running past its end: rejected, not clamped.
    expect_err(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 42, 1)),
        OUT_OF_RANGE,
    );
    expect_err(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 32, 10)),
        OUT_OF_RANGE,
    );
    expect_err(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 0, 42)),
        OUT_OF_RANGE,
    );
    // The bytes do not fit one response: the next-ID bytes plus 23 bytes are 25.
    expect_err(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 0, 23)),
        CANT_RETURN,
    );
    // FFh on a record longer than one response carries (sensor 0: 41 bytes).
    expect_err(
        se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 0, 0xFF)),
        CANT_RETURN,
    );
    // Exactly one full response: 22 record bytes.
    let r = se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 0, 22));
    assert_eq!((r.cc, r.data.len()), (OK, 24));
    // Offset equal to the record length, count 0: nothing to return, still the next ID.
    let r = se(&mut c, GET_DEVICE_SDR, &sdr_request(res, 0, 41, 0));
    assert_eq!((r.cc, r.data), (OK, vec![0x01, 0x00]));
}

// ---- Get Sensor Reading (Table 35-15; VITA Tables 10.2.1-1 .. 10.2.7-1) ----

fn reading(c: &mut Context, sensor: u8) -> Vec<u8> {
    let r = se(c, GET_SENSOR_READING, &[sensor]);
    assert_eq!(r.cc, OK, "sensor {sensor}");
    r.data
}

fn snapshot(
    health: Summary,
    voltage: Summary,
    temp_flags: u8,
    power: PowerState,
) -> SensorSnapshot {
    let mut s = SensorSnapshot::EMPTY;
    s.health = health;
    s.voltage = voltage;
    s.temperature.available = true;
    s.temperature.flags = temp_flags;
    s.power = power;
    s.rail_count = 1;
    s.rails[0].status = RailStatus::InRange;
    s
}

#[test]
fn sensor_zero_reports_the_fru_state_mask() {
    let mut c = ctx();
    // Power-up M0: bit 0 of the Current State Mask.
    assert_eq!(reading(&mut c, 0), [0x00, scanning(), 0x01]);
    let on = snapshot(Summary::Good, Summary::Good, 0, PowerState::On);
    // Checks pass: M0 -> M1 (bit 1); payload power valid: M1 -> M4 (bit 4).
    c.update_sensors(1000, &on, &mut NoTrace);
    assert_eq!(
        (c.fru_state.state(), reading(&mut c, 0)),
        (State::M1, vec![0x00, scanning(), 0x02])
    );
    c.update_sensors(2000, &on, &mut NoTrace);
    assert_eq!(
        (c.fru_state.state(), reading(&mut c, 0)),
        (State::M4, vec![0x00, scanning(), 0x10])
    );
    // Power lost: M4 -> M6 (bit 6) -> M1.
    let off = snapshot(Summary::Good, Summary::Good, 0, PowerState::Off);
    c.update_sensors(3000, &off, &mut NoTrace);
    assert_eq!(reading(&mut c, 0), [0x00, scanning(), 0x40]);
    c.update_sensors(4000, &off, &mut NoTrace);
    assert_eq!(reading(&mut c, 0), [0x00, scanning(), 0x02]);
}

#[test]
fn sensor_one_reports_the_link_view() {
    let mut c = ctx_with_b();
    // Power-up: A local control and enabled (bit 3 of byte 2; byte 4 bit 1 A en / B dis).
    assert_eq!(reading(&mut c, 1), [0x08, scanning(), 0x02]);
    // The Link's view changes: both buses in local control and enabled.
    c.link.buses.b.mode = ipmc_core::link::BusMode::LocalControl;
    assert_eq!(reading(&mut c, 1), [0x88, scanning(), 0x08]);
}

#[test]
fn sensors_two_to_four_follow_the_snapshot() {
    let mut c = ctx();
    // Before the first sample nothing is available: status bit 5.
    for sensor in [2, 3, 4] {
        assert_eq!(
            reading(&mut c, sensor),
            [0x00, unavailable(), 0x00],
            "sensor {sensor}"
        );
    }
    c.update_sensors(
        10,
        &snapshot(Summary::Good, Summary::Good, 0, PowerState::Off),
        &mut NoTrace,
    );
    assert_eq!(reading(&mut c, 2), [0x00, scanning(), 0x01]);
    assert_eq!(reading(&mut c, 3), [0x00, scanning(), 0x01]);
    assert_eq!(reading(&mut c, 4), [0x00, scanning(), 0x00]);
    c.update_sensors(
        20,
        &snapshot(Summary::Bad, Summary::Bad, TEMP_FLAG_UC, PowerState::Off),
        &mut NoTrace,
    );
    assert_eq!(reading(&mut c, 2), [0x00, scanning(), 0x02]);
    assert_eq!(reading(&mut c, 3), [0x00, scanning(), 0x02]);
    assert_eq!(reading(&mut c, 4), [0x00, scanning(), 0x10]);
}

#[test]
fn sensors_five_and_six_report_no_test_run() {
    let mut c = ctx();
    // Payload Test Results: reading unavailable (bit 5), result byte ignored.
    assert_eq!(reading(&mut c, 5), [0x00, unavailable(), 0x00]);
    // Payload Test Status: 01b, test not in progress.
    assert_eq!(reading(&mut c, 6), [0x00, scanning(), 0x01]);
}

#[test]
fn read_sensor_serves_the_handler_and_the_console_from_one_function() {
    let c = ctx_with_b();
    let mut out = [0u8; 3];
    // The caller chooses the link state the IPMB Link sensor reports.
    let mut both = c.link.buses;
    both.b.mode = BusMode::LocalControl;
    assert_eq!(c.read_sensor(1, &c.link.buses, &mut out), Ok(3));
    assert_eq!(out, [0x08, scanning(), 0x02]);
    assert_eq!(c.read_sensor(1, &both, &mut out), Ok(3));
    assert_eq!(out, [0x88, scanning(), 0x08]);
    // The other sensors follow the context.
    assert_eq!(c.read_sensor(0, &both, &mut out), Ok(3));
    assert_eq!(out, [0x00, scanning(), 0x01]);
    assert_eq!(
        c.read_sensor(SENSOR_COUNT, &both, &mut out),
        Err(SensorReadError::NotPresent)
    );
    assert_eq!(
        c.read_sensor(0, &both, &mut out[..2]),
        Err(SensorReadError::OutputTooSmall)
    );
}

#[test]
fn reading_status_bit_7_follows_the_sensors_event_enable() {
    let mut c = ctx_with_b();
    let both = c.link.buses;
    let mut out = [0u8; 3];
    // Power-up: events off everywhere at Tier-1, on for every sensor at Tier-2/3.
    let power_up = by_tier([0x00, 0x80, 0x80]);
    for sensor in 0..=6u8 {
        assert_eq!(c.read_sensor(sensor, &both, &mut out), Ok(3));
        assert_eq!(out[1] & 0xC0, 0x40 | power_up, "sensor {sensor}");
    }
    // Switching one sensor sets or clears bit 7 of that sensor only.
    for (enabled, bit) in [(true, 0x80), (false, 0x00)] {
        c.event_enables.get_mut(2).unwrap().enabled = enabled;
        for (sensor, status) in [(0u8, power_up), (2, bit), (3, power_up), (6, power_up)] {
            assert_eq!(c.read_sensor(sensor, &both, &mut out), Ok(3));
            assert_eq!(out[1] & 0x80, status, "sensor {sensor} enabled={enabled}");
        }
    }
}

#[test]
fn sensor_reading_error_paths() {
    let mut c = ctx();
    // The first numbers past the sensors of the Tier (7, or 8 with the Payload Mode sensor).
    for sensor in [SENSOR_COUNT, SENSOR_COUNT + 1, 0x80, 0xFF] {
        expect_err(se(&mut c, GET_SENSOR_READING, &[sensor]), NOT_PRESENT);
    }
    expect_err(se(&mut c, GET_SENSOR_READING, &[]), BAD_CMD_LEN);
    expect_err(se(&mut c, GET_SENSOR_READING, &[0, 0]), BAD_CMD_LEN);
}

// ---- Get VSO Capabilities (VITA Table 10.1.3.1-1) ----

#[test]
fn vso_capabilities_for_one_and_two_interfaces() {
    // Layer 1 / Tier-1 = 00h (Tier-2 01h, Tier-3 02h); 100 kHz (400 kHz, 01b in bits 5:4, at
    // Tier-3) and one interface = 00h; VITA 46.11 = 00h; 2022 = 22h; max FRU Device ID 0; IPMC
    // FRU Device ID 0.
    let tier = by_tier([0x00, 0x01, 0x02]);
    let speed = by_tier([0x00, 0x00, 0x10]);
    let one = call_vso(&mut ctx(), GET_VSO_CAPABILITIES, &[]);
    assert_eq!(
        (one.cc, one.data),
        (OK, vec![tier, speed, 0x00, 0x22, 0x00, 0x00])
    );
    // IPMB-B fitted: number of interfaces 01b (Rule 4.1.1-2).
    let two = call_vso(&mut ctx_with_b(), GET_VSO_CAPABILITIES, &[]);
    assert_eq!(
        (two.cc, two.data),
        (OK, vec![tier, speed | 0x01, 0x00, 0x22, 0x00, 0x00])
    );
}

#[test]
fn vso_capabilities_frame_is_byte_exact() {
    // rqSA 20h, netFn 2Dh<<2 = B4h, chk1 -(20+B4) = 2Ch, rsSA 82h, seq 5<<2 = 14h, cmd 00h,
    // CC 00h, data 03 TT SS 00 22 00 00 with the IPMC identifier TT = Tier - 1 (00h, 01h, 02h)
    // and the IPMB capabilities SS = 00h (10h, 400 kHz, at Tier-3), chk2 =
    // -(82+14+03+TT+SS+22) = 45h, 44h, 33h.
    let r = call(&mut ctx(), 0x2C, GET_VSO_CAPABILITIES, &[0x03]);
    let (tier, speed, chk2) = by_tier([(0x00, 0x00, 0x45), (0x01, 0x00, 0x44), (0x02, 0x10, 0x33)]);
    assert_eq!(
        r.frame,
        [
            0x20, 0xB4, 0x2C, 0x82, 0x14, 0x00, 0x00, 0x03, tier, speed, 0x00, 0x22, 0x00, 0x00,
            chk2
        ]
    );
}

#[test]
fn vso_capabilities_error_paths() {
    let mut c = ctx();
    expect_err(call_vso(&mut c, GET_VSO_CAPABILITIES, &[0x00]), BAD_CMD_LEN);
    // Another defining body is C1h from the dispatcher, echoing that body's byte; no defining
    // body byte at all is C7h with no data (IPMI v2.0 Tables 5-1 and 5-2).
    let r = call(&mut c, 0x2C, GET_VSO_CAPABILITIES, &[0x05]);
    assert_eq!((r.cc, r.data), (0xC1, vec![0x05]));
    let r = call(&mut c, 0x2C, GET_VSO_CAPABILITIES, &[]);
    assert_eq!((r.cc, r.data), (0xC7, vec![]));
}

// ---- Get Device Locator Record ID (Table 10.1.3.11-1) ----

// ---- Get FRU Address Info (Table 10.1.3.3-1) ----

#[test]
fn fru_address_info_for_the_ifru() {
    // GA 1: hardware address 41h, IPMB address 82h, reserved FFh, FRU Device ID 0, site number 1
    // (GA[4:0], Rule 4.1.4.3-3), site type 00h (Front Loading VPX Plug-In Module, Table 4.1.4.3-1).
    let expected = vec![0x41, 0x82, 0xFF, 0x00, 0x01, 0x00];
    let mut c = ctx();
    for request in [&[][..], &[0x00][..], &[0x00, 0x03, 0x01, 0x00][..]] {
        let r = call_vso(&mut c, GET_FRU_ADDRESS_INFO, request);
        assert_eq!((r.cc, r.data), (OK, expected.clone()), "{request:02X?}");
    }
}

#[test]
fn fru_address_info_follows_the_geographic_address() {
    // GA 5 with GAP set for odd parity: logic word 100101b, pins at the opposite level.
    let mut c = ctx();
    c.addr = addr::decode(0x1A);
    assert_eq!((c.addr.ga, c.addr.parity_ok), (5, true));
    let r = call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[]);
    // Hardware address 40h + 5, IPMB address 8Ah, site number 5.
    assert_eq!(
        (r.cc, r.data),
        (OK, vec![0x45, 0x8A, 0xFF, 0x00, 0x05, 0x00])
    );
    // The physical address key is now site number 5.
    let r = call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[0, 0x03, 0x05, 0x00]);
    assert_eq!(r.cc, OK);
    expect_err(
        call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[0, 0x03, 0x01, 0x00]),
        DEST_UNAVAILABLE,
    );
}

#[test]
fn fru_address_info_error_paths() {
    let mut c = ctx();
    for bad in [&[0, 0][..], &[0, 3, 1][..], &[0, 3, 1, 0, 0][..]] {
        expect_err(call_vso(&mut c, GET_FRU_ADDRESS_INFO, bad), BAD_CMD_LEN);
    }
    // A Subsidiary FRU: Rule 4.2-3.
    expect_err(call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[1]), OUT_OF_RANGE);
    // Only key type 03h (Physical Address) is defined.
    for key_type in [0x00, 0x01, 0x02, 0x04] {
        expect_err(
            call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[0, key_type, 1, 0]),
            BAD_FIELD,
        );
    }
    // No FRU at another site number or site type: Rule 4.1.1-18.
    expect_err(
        call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[0, 3, 2, 0]),
        DEST_UNAVAILABLE,
    );
    expect_err(
        call_vso(&mut c, GET_FRU_ADDRESS_INFO, &[0, 3, 1, 7]),
        DEST_UNAVAILABLE,
    );
}

// ---- Set IPMB State at the handler (the Link applies the change: tier1_link.rs) ----

#[test]
fn set_ipmb_state_leaves_a_validated_change_pending() {
    let mut c = ctx_with_b();
    // 03 01 01: IPMB-A and IPMB-B Local Control (the capture's request).
    let r = call_vso(&mut c, SET_IPMB_STATE, &[0x01, 0x01]);
    assert_eq!((r.cc, r.data), (OK, vec![]));
    let pending = c.pending_ipmb.take().expect("a pending change");
    assert_eq!(
        (pending.a, pending.b),
        (
            Some(ipmc_core::link::BusMode::LocalControl),
            Some(ipmc_core::link::BusMode::LocalControl)
        )
    );
    // The handler does not touch the view; the Link applies the change.
    assert_eq!(c.link, ctx_with_b().link);
}

#[test]
fn set_ipmb_state_refusals_change_nothing() {
    let mut c = ctx_with_b();
    // Link identification other than 00h, on either bus.
    for request in [&[0x03, 0x01][..], &[0x01, 0x03][..], &[0x80, 0xFF][..]] {
        expect_err(call_vso(&mut c, SET_IPMB_STATE, request), BAD_FIELD);
    }
    // IPMB Speed: the reserved values 10b and 11b of either bus and a reserved nibble (400 kHz is
    // `speed_byte_400_khz_follows_the_tier`).
    for speed in [0x02, 0x08, 0x03, 0x0C, 0x10, 0x11] {
        expect_err(
            call_vso(&mut c, SET_IPMB_STATE, &[0x01, 0x01, speed]),
            BAD_FIELD,
        );
    }
    // Isolating IPMB-A while IPMB-B is isolated would leave no bus (Rule 4.1.8-3).
    expect_err(call_vso(&mut c, SET_IPMB_STATE, &[0x00, 0xFF]), BAD_FIELD);
    expect_err(call_vso(&mut c, SET_IPMB_STATE, &[0x00, 0x00]), BAD_FIELD);
    // Lengths.
    for bad in [&[][..], &[0x01][..], &[0x01, 0x01, 0x00, 0x00][..]] {
        expect_err(call_vso(&mut c, SET_IPMB_STATE, bad), BAD_CMD_LEN);
    }
    assert!(
        c.pending_ipmb.is_none(),
        "no refused request leaves a change"
    );
    // IPMB-B not fitted: Local Control for it is unsupported (Rule 4.1.1-14).
    let mut single = ctx();
    expect_err(
        call_vso(&mut single, SET_IPMB_STATE, &[0xFF, 0x01]),
        BAD_FIELD,
    );
    assert!(single.pending_ipmb.is_none());
    // ... but IPMB-A in Local Control and speed 00h is accepted, and FFh FFh changes nothing.
    assert_eq!(
        call_vso(&mut single, SET_IPMB_STATE, &[0x01, 0xFF, 0x00]).cc,
        OK
    );
    assert_eq!(call_vso(&mut single, SET_IPMB_STATE, &[0xFF, 0xFF]).cc, OK);
}

#[test]
fn speed_byte_400_khz_follows_the_tier() {
    // 400 kHz (01b) for A, for B (01b << 2) and both: refused below Tier-3 (Rule 4.1.1-4.4),
    // accepted at Tier-3 with the change handed to the Link.
    let accepted = by_tier([false, false, true]);
    for speed in [0x01u8, 0x04, 0x05] {
        let mut c = ctx_with_b();
        let r = call_vso(&mut c, SET_IPMB_STATE, &[0x01, 0x01, speed]);
        if accepted {
            assert_eq!(r.cc, OK, "speed {speed:02X}");
            let pending = c.pending_ipmb.take().expect("a pending change");
            let fast = |bit: u8| {
                (speed & bit != 0)
                    .then_some(ipmc_core::link::BusSpeed::Fast)
                    .or(Some(ipmc_core::link::BusSpeed::Standard))
            };
            assert_eq!((pending.speed_a, pending.speed_b), (fast(0x01), fast(0x04)));
        } else {
            assert_eq!(r.cc, BAD_FIELD, "speed {speed:02X}");
            assert!(c.pending_ipmb.is_none());
        }
    }
    // Without the speed byte no speed changes at any Tier.
    let mut c = ctx_with_b();
    assert_eq!(call_vso(&mut c, SET_IPMB_STATE, &[0x01, 0x01]).cc, OK);
    let pending = c.pending_ipmb.take().unwrap();
    assert_eq!((pending.speed_a, pending.speed_b), (None, None));
    // 400 kHz for IPMB-B when it is not fitted: CCh even at Tier-3.
    let mut single = ctx();
    expect_err(
        call_vso(&mut single, SET_IPMB_STATE, &[0xFF, 0xFF, 0x04]),
        BAD_FIELD,
    );
}
