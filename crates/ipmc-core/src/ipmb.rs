//! ipmb.rs - IPMB frame codec: checksums, parser, request and response encoders.
//!
//! Frames use 8-bit addresses; byte 0 is the destination slave address as it appears on the wire.
//!
//! ```text
//! Request:  rsSA, netFn<<2|rsLUN, chk1, rqSA, rqSeq<<2|rqLUN, cmd, data..., chk2
//! Response: rqSA, netFn<<2|rqLUN, chk1, rsSA, rqSeq<<2|rsLUN, cmd, completion code, data..., chk2
//! ```
//!
//! chk1 covers bytes 0..1 (sum of bytes 0..2 is 0 mod 256). chk2 covers bytes 3..n-2
//! (sum of bytes 3..n-1 is 0 mod 256). A message is a response when its netFn is odd.
//! Verified against the captured vectors (vectors/sbc_ab.vec) by tests/trace_vectors.rs.

/// IPMB_MIN_REQUEST_LEN -- smallest request: 6 header bytes plus chk2 (IPMI v2.0 IPMB message format).
pub const IPMB_MIN_REQUEST_LEN: usize = 7;
/// IPMB_MIN_RESPONSE_LEN -- smallest response: 6 header bytes, completion code and chk2.
pub const IPMB_MIN_RESPONSE_LEN: usize = 8;
/// IPMB_MAX_LEN -- largest frame, from ipmi_defs.h IPMB_MSG_MAX (peer: the firmware's I2C receive
/// buffer, bsp/i2c_hw.rs RX_FRAME_MAX, is this plus the address byte).
pub const IPMB_MAX_LEN: usize = ipmc_defs::IPMB_MSG_MAX as usize;
/// IPMB_RESPONSE_DATA_MAX -- largest data field of a response: the frame minus header, completion code and chk2.
pub const IPMB_RESPONSE_DATA_MAX: usize = IPMB_MAX_LEN - IPMB_MIN_RESPONSE_LEN;
/// IPMB_REQUEST_DATA_MAX -- largest data field of a request: the frame minus the six header bytes and chk2.
pub const IPMB_REQUEST_DATA_MAX: usize = IPMB_MAX_LEN - IPMB_MIN_REQUEST_LEN;

/// NETFN_APP -- NetFn Application (request; the response is 07h). IPMI v2.0 Table 5-1.
pub const NETFN_APP: u8 = 0x06;
/// NETFN_SE -- NetFn Sensor/Event (request; the response is 05h). IPMI v2.0 Table 5-1.
pub const NETFN_SE: u8 = 0x04;
/// NETFN_STORAGE -- NetFn Storage (request; the response is 0Bh). IPMI v2.0 Table 5-1.
pub const NETFN_STORAGE: u8 = 0x0A;
/// NETFN_GROUP_EXT -- NetFn Group Extension (request; the response is 2Dh). IPMI v2.0 Table 5-1 and section 5.1;
/// VITA 46.11 section 10.1.2 carries its commands here.
pub const NETFN_GROUP_EXT: u8 = 0x2C;

/// Offset of the source address, the first byte chk2 covers.
const OFF_SRC: usize = 3;
/// First byte after cmd: request data, or the completion code of a response.
const OFF_BODY: usize = 6;

// `parse` destructures the six header bytes with a slice pattern; these tie it to the constants.
const _: () = assert!(OFF_BODY + 1 == IPMB_MIN_REQUEST_LEN, "header plus chk2");
const _: () = assert!(
    IPMB_MIN_REQUEST_LEN + 1 == IPMB_MIN_RESPONSE_LEN,
    "plus completion code"
);

const LUN_MASK: u8 = 0x03;
const LUN_BITS: u8 = 2;
const NETFN_MAX: u8 = 0x3F;
const SEQ_MAX: u8 = 0x3F;
/// Odd netFn marks a response.
const NETFN_RESPONSE_BIT: u8 = 0x01;
/// NETFN_SHIFT -- the NetFn sits in bits 7:2 of byte 1 (the other two bits are the LUN).
const NETFN_SHIFT: u32 = 2;
/// Bit 0 of a slave address byte is the R/W bit of the I2C address phase; an IPMB frame
/// carries 8-bit addresses with bit 0 clear (IPMI v2.0 section 6.3 message format).
const ADDRESS_RW_BIT: u8 = 0x01;

/// IpmbError -- codec failure; each variant names one rejected condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpmbError {
    /// Frame shorter than the minimum for its kind.
    TooShort,
    /// Frame longer than IPMB_MSG_MAX.
    TooLong,
    /// chk1 does not match bytes 0..1.
    HeaderChecksum,
    /// chk2 does not match bytes 3..n-2.
    DataChecksum,
    /// Output buffer cannot hold the frame.
    OutBufferTooSmall,
    /// A request was required but the netFn is odd (response).
    NotARequest,
    /// A response was required but the netFn is even (request).
    NotAResponse,
    /// A header field is out of range (netFn or seq above 6 bits, LUN above 2 bits).
    FieldRange,
    /// A destination or source address byte has bit 0 set (reserved R/W position).
    InvalidAddress,
}

impl IpmbError {
    /// name -- log name of the error; returns a static string.
    pub const fn name(self) -> &'static str {
        match self {
            IpmbError::TooShort => "TooShort",
            IpmbError::TooLong => "TooLong",
            IpmbError::HeaderChecksum => "HeaderChecksum",
            IpmbError::DataChecksum => "DataChecksum",
            IpmbError::OutBufferTooSmall => "OutBufferTooSmall",
            IpmbError::NotARequest => "NotARequest",
            IpmbError::NotAResponse => "NotAResponse",
            IpmbError::FieldRange => "FieldRange",
            IpmbError::InvalidAddress => "InvalidAddress",
        }
    }
}

/// Header -- addressing and command fields of a frame, in destination/source terms.
/// For a request dest = rsSA, src = rqSA; for a response dest = rqSA, src = rsSA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Destination 8-bit slave address (byte 0).
    pub dest: u8,
    /// Network function (6 bits); odd for a response.
    pub netfn: u8,
    /// LUN in byte 1 (rsLUN in a request, rqLUN in a response).
    pub dest_lun: u8,
    /// Source 8-bit slave address (byte 3).
    pub src: u8,
    /// Request sequence number (6 bits).
    pub seq: u8,
    /// LUN in byte 4 (rqLUN in a request, rsLUN in a response).
    pub src_lun: u8,
    /// Command code.
    pub cmd: u8,
}

/// is_request_frame -- true when the raw frame's NetFn (byte 1, bits 7:2) is even: a request.
/// The one classification of a frame by its bytes, for frames `parse` may have rejected: an even
/// NetFn is a request, an odd one a response (IPMB message format); `Header::is_response` is the
/// same rule on a parsed header. False for a frame too short to hold a NetFn.
pub fn is_request_frame(bytes: &[u8]) -> bool {
    bytes
        .get(1)
        .is_some_and(|b| (b >> NETFN_SHIFT) & NETFN_RESPONSE_BIT == 0)
}

impl Header {
    /// is_response -- true when the netFn is odd.
    pub const fn is_response(&self) -> bool {
        self.netfn & NETFN_RESPONSE_BIT != 0
    }

    /// validate -- checks the addresses and field ranges; returns InvalidAddress when an
    /// address byte has bit 0 set, FieldRange when a field does not fit its bits.
    fn validate(&self) -> Result<(), IpmbError> {
        if (self.dest | self.src) & ADDRESS_RW_BIT != 0 {
            return Err(IpmbError::InvalidAddress);
        }
        let in_range = self.netfn <= NETFN_MAX
            && self.seq <= SEQ_MAX
            && self.dest_lun <= LUN_MASK
            && self.src_lun <= LUN_MASK;
        if in_range {
            Ok(())
        } else {
            Err(IpmbError::FieldRange)
        }
    }
}

/// Frame -- A parsed frame; `data` borrows the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    /// Addressing and command fields (bytes 0, 1, 3, 4, 5).
    pub header: Header,
    /// Completion code; Some for responses only.
    pub completion_code: Option<u8>,
    /// Data field, without completion code and checksums.
    pub data: &'a [u8],
}

// The IPMB frame checksum (IPMI v2.0 section 13.8) and the FRU zero checksum (Platform Management
// FRU Information Storage Definition, sections 8, 11 and 12) are one function, kept in the
// lowest crate that needs it (ipmc-fru builds the FRU image at compile time).
pub use ipmc_wire::zero_checksum as checksum;

/// parse -- validates and splits a received frame.
/// Checks length, then chk1, then the address bytes (bit 0 clear), then the response minimum,
/// then chk2; rejects, never clamps.
/// Returns the Frame (data borrowed from `frame`) or the first error found.
pub fn parse(frame: &[u8]) -> Result<Frame<'_>, IpmbError> {
    if frame.len() > IPMB_MAX_LEN {
        return Err(IpmbError::TooLong);
    }
    // At least IPMB_MIN_REQUEST_LEN bytes: six header bytes, then the body ending in chk2.
    let [dest, netfn_lun, chk1, src, seq_lun, cmd, body @ ..] = frame else {
        return Err(IpmbError::TooShort);
    };
    let Some((chk2, body)) = body.split_last() else {
        return Err(IpmbError::TooShort);
    };
    if checksum(&[*dest, *netfn_lun]) != *chk1 {
        return Err(IpmbError::HeaderChecksum);
    }
    if (dest | src) & ADDRESS_RW_BIT != 0 {
        return Err(IpmbError::InvalidAddress);
    }
    let header = Header {
        dest: *dest,
        netfn: netfn_lun >> LUN_BITS,
        dest_lun: netfn_lun & LUN_MASK,
        src: *src,
        seq: seq_lun >> LUN_BITS,
        src_lun: seq_lun & LUN_MASK,
        cmd: *cmd,
    };
    let (completion_code, data) = if header.is_response() {
        let Some((code, data)) = body.split_first() else {
            return Err(IpmbError::TooShort);
        };
        (Some(*code), data)
    } else {
        (None, body)
    };
    let covered = frame
        .get(OFF_SRC..frame.len() - 1)
        .ok_or(IpmbError::TooShort)?;
    if checksum(covered) != *chk2 {
        return Err(IpmbError::DataChecksum);
    }
    Ok(Frame {
        header,
        completion_code,
        data,
    })
}

/// Sequential byte writer over the output buffer; every push is bounds-checked.
struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Writer<'_> {
    /// push -- appends one byte; returns OutBufferTooSmall when the buffer is full.
    fn push(&mut self, value: u8) -> Result<(), IpmbError> {
        let slot = self
            .buf
            .get_mut(self.pos)
            .ok_or(IpmbError::OutBufferTooSmall)?;
        *slot = value;
        self.pos += 1;
        Ok(())
    }

    /// push_all -- appends a byte slice; returns OutBufferTooSmall when it does not fit.
    fn push_all(&mut self, values: &[u8]) -> Result<(), IpmbError> {
        for value in values {
            self.push(*value)?;
        }
        Ok(())
    }

    /// written -- the bytes pushed so far.
    fn written(&self) -> &[u8] {
        self.buf.get(..self.pos).unwrap_or(&[])
    }
}

/// write_frame -- the one frame writer behind encode_request and encode_response.
/// `completion_code` is Some for responses. Returns the frame length written.
fn write_frame(
    header: &Header,
    completion_code: Option<u8>,
    data: &[u8],
    out: &mut [u8],
) -> Result<usize, IpmbError> {
    header.validate()?;
    let code_len = usize::from(completion_code.is_some());
    let total = OFF_BODY + code_len + data.len() + 1;
    if total > IPMB_MAX_LEN {
        return Err(IpmbError::TooLong);
    }
    if out.len() < total {
        return Err(IpmbError::OutBufferTooSmall);
    }
    let netfn_lun = (header.netfn << LUN_BITS) | header.dest_lun;
    let mut w = Writer { buf: out, pos: 0 };
    w.push(header.dest)?;
    w.push(netfn_lun)?;
    w.push(checksum(&[header.dest, netfn_lun]))?;
    w.push(header.src)?;
    w.push((header.seq << LUN_BITS) | header.src_lun)?;
    w.push(header.cmd)?;
    if let Some(code) = completion_code {
        w.push(code)?;
    }
    w.push_all(data)?;
    let covered = w.written().get(OFF_SRC..).ok_or(IpmbError::TooShort)?;
    let chk2 = checksum(covered);
    w.push(chk2)?;
    Ok(w.pos)
}

/// encode_request -- writes a request frame into `out`.
/// Returns the frame length, or NotARequest (odd netFn), FieldRange, TooLong, OutBufferTooSmall.
pub fn encode_request(header: &Header, data: &[u8], out: &mut [u8]) -> Result<usize, IpmbError> {
    if header.is_response() {
        return Err(IpmbError::NotARequest);
    }
    write_frame(header, None, data, out)
}

/// encode_response -- writes a response frame (with completion code) into `out`.
/// Returns the frame length, or NotAResponse (even netFn), FieldRange, TooLong, OutBufferTooSmall.
pub fn encode_response(
    header: &Header,
    completion_code: u8,
    data: &[u8],
    out: &mut [u8],
) -> Result<usize, IpmbError> {
    if !header.is_response() {
        return Err(IpmbError::NotAResponse);
    }
    write_frame(header, Some(completion_code), data, out)
}

/// response_header_for -- builds the response addressing for a request.
/// Swaps the addresses and the LUNs, netFn + 1, same seq and cmd.
/// Returns the Header, or NotARequest when `request` is a response.
pub fn response_header_for(request: &Frame) -> Result<Header, IpmbError> {
    let req = &request.header;
    if req.is_response() {
        return Err(IpmbError::NotARequest);
    }
    Ok(Header {
        dest: req.src,
        // The request netFn is even, so setting bit 0 is netFn + 1 and cannot overflow.
        netfn: req.netfn | NETFN_RESPONSE_BIT,
        dest_lun: req.src_lun,
        src: req.dest,
        seq: req.seq,
        src_lun: req.dest_lun,
        cmd: req.cmd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Get Device ID style request from the CM (0x20) to 0x82, seq 5.
    const REQ: [u8; 7] = [0x82, 0x18, 0x66, 0x20, 0x14, 0x01, 0xCB];
    const REQ_HEADER: Header = Header {
        dest: 0x82,
        netfn: 6,
        dest_lun: 0,
        src: 0x20,
        seq: 5,
        src_lun: 0,
        cmd: 1,
    };

    #[test]
    fn request_round_trip() {
        let frame = parse(&REQ).unwrap();
        assert_eq!(frame.header, REQ_HEADER);
        assert_eq!(frame.completion_code, None);
        assert!(frame.data.is_empty());
        let mut out = [0u8; IPMB_MAX_LEN];
        let n = encode_request(&frame.header, frame.data, &mut out).unwrap();
        assert_eq!(&out[..n], &REQ);
    }

    #[test]
    fn response_round_trip_with_data() {
        let hdr = response_header_for(&parse(&REQ).unwrap()).unwrap();
        assert_eq!(hdr.dest, 0x20);
        assert_eq!(hdr.netfn, 7);
        assert_eq!(hdr.src, 0x82);
        assert_eq!(hdr.seq, 5);
        let mut out = [0u8; IPMB_MAX_LEN];
        let n = encode_response(&hdr, 0x00, &[1, 2, 3], &mut out).unwrap();
        let frame = parse(&out[..n]).unwrap();
        assert_eq!(frame.completion_code, Some(0));
        assert_eq!(frame.data, &[1, 2, 3]);
        assert_eq!(frame.header, hdr);
        let mut again = [0u8; IPMB_MAX_LEN];
        let m = encode_response(&frame.header, 0, frame.data, &mut again).unwrap();
        assert_eq!(&again[..m], &out[..n]);
    }

    #[test]
    fn raw_frames_are_classified_by_the_netfn_parity() {
        // NetFn 04h (even) is a request, 05h a response; the LUN bits do not matter.
        assert!(is_request_frame(&[0x20, 0x10, 0xD0]));
        assert!(is_request_frame(&[0x20, 0x11, 0xD0]));
        assert!(!is_request_frame(&[0x20, 0x14, 0xD0]));
        assert!(!is_request_frame(&[0x20, 0x17, 0xD0]));
        assert!(!is_request_frame(&[0x20]), "too short for a NetFn");
        assert!(!is_request_frame(&[]));
    }

    #[test]
    fn luns_swap_in_response_header() {
        let hdr = Header {
            dest_lun: 2,
            src_lun: 1,
            ..REQ_HEADER
        };
        let mut out = [0u8; IPMB_MAX_LEN];
        let n = encode_request(&hdr, &[], &mut out).unwrap();
        let resp = response_header_for(&parse(&out[..n]).unwrap()).unwrap();
        assert_eq!((resp.dest_lun, resp.src_lun), (1, 2));
    }

    #[test]
    fn error_too_short() {
        assert_eq!(parse(&REQ[..6]), Err(IpmbError::TooShort));
        // A response needs 8 bytes: build a valid 7-byte frame with an odd netFn.
        let mut f = [0x20, 0x1C, 0, 0x82, 0x14, 0x01, 0];
        f[2] = checksum(&f[..2]);
        f[6] = checksum(&f[3..6]);
        assert_eq!(parse(&f), Err(IpmbError::TooShort));
    }

    #[test]
    fn error_too_long() {
        assert_eq!(parse(&[0u8; IPMB_MAX_LEN + 1]), Err(IpmbError::TooLong));
        // One byte over with both checksums valid: the length alone rejects it.
        let mut f = [0u8; IPMB_MAX_LEN + 1];
        f[..OFF_BODY].copy_from_slice(&REQ[..OFF_BODY]);
        f[IPMB_MAX_LEN] = checksum(&f[OFF_SRC..IPMB_MAX_LEN]);
        assert_eq!(parse(&f), Err(IpmbError::TooLong));
    }

    #[test]
    fn boundary_lengths_parse() {
        let resp_hdr = response_header_for(&parse(&REQ).unwrap()).unwrap();
        let mut out = [0u8; IPMB_MAX_LEN];
        // Shortest response (8 bytes, no data) and longest request and response (32 bytes).
        let n = encode_response(&resp_hdr, 0, &[], &mut out).unwrap();
        assert_eq!(n, IPMB_MIN_RESPONSE_LEN);
        assert!(parse(&out[..n]).unwrap().data.is_empty());
        let data = [0x5Au8; IPMB_MAX_LEN - IPMB_MIN_REQUEST_LEN];
        let n = encode_request(&REQ_HEADER, &data, &mut out).unwrap();
        assert_eq!(parse(&out[..n]).unwrap().data, &data[..]);
        let data = [0xA5u8; IPMB_RESPONSE_DATA_MAX];
        let n = encode_response(&resp_hdr, 0, &data, &mut out).unwrap();
        assert_eq!(n, IPMB_MAX_LEN);
        assert_eq!(parse(&out[..n]).unwrap().data, &data[..]);
    }

    #[test]
    fn error_checksums() {
        let mut bad = REQ;
        bad[2] ^= 1;
        assert_eq!(parse(&bad), Err(IpmbError::HeaderChecksum));
        let mut bad = REQ;
        bad[6] ^= 1;
        assert_eq!(parse(&bad), Err(IpmbError::DataChecksum));
        // The data checksum starts at byte 3: changing byte 0 and chk1 together stays valid.
        let mut moved = REQ;
        moved[0] = 0x84;
        moved[2] = checksum(&moved[..2]);
        assert!(parse(&moved).is_ok());
    }

    #[test]
    fn error_out_buffer_too_small() {
        let mut out = [0u8; 6];
        assert_eq!(
            encode_request(&REQ_HEADER, &[], &mut out),
            Err(IpmbError::OutBufferTooSmall)
        );
    }

    #[test]
    fn error_frame_too_long_to_encode() {
        let mut out = [0u8; 64];
        let data = [0u8; IPMB_MAX_LEN];
        assert_eq!(
            encode_request(&REQ_HEADER, &data, &mut out),
            Err(IpmbError::TooLong)
        );
        let exact = [0u8; IPMB_MAX_LEN - IPMB_MIN_REQUEST_LEN];
        assert_eq!(
            encode_request(&REQ_HEADER, &exact, &mut out),
            Ok(IPMB_MAX_LEN)
        );
    }

    #[test]
    fn error_wrong_kind() {
        let mut out = [0u8; IPMB_MAX_LEN];
        let resp_hdr = Header {
            netfn: 7,
            ..REQ_HEADER
        };
        assert_eq!(
            encode_request(&resp_hdr, &[], &mut out),
            Err(IpmbError::NotARequest)
        );
        assert_eq!(
            encode_response(&REQ_HEADER, 0, &[], &mut out),
            Err(IpmbError::NotAResponse)
        );
        let n = encode_response(&resp_hdr, 0, &[], &mut out).unwrap();
        assert_eq!(
            response_header_for(&parse(&out[..n]).unwrap()),
            Err(IpmbError::NotARequest)
        );
    }

    #[test]
    fn error_field_range() {
        let mut out = [0u8; IPMB_MAX_LEN];
        for bad in [
            Header {
                seq: 0x40,
                ..REQ_HEADER
            },
            Header {
                dest_lun: 4,
                ..REQ_HEADER
            },
            Header {
                src_lun: 4,
                ..REQ_HEADER
            },
            Header {
                netfn: 0x40,
                ..REQ_HEADER
            },
        ] {
            assert_eq!(
                encode_request(&bad, &[], &mut out),
                Err(IpmbError::FieldRange)
            );
        }
    }

    // Frames typed byte by byte from the IPMB layout (rsSA, netFn<<2|rsLUN, chk1, rqSA,
    // rqSeq<<2|rqLUN, cmd, data, chk2); the encoder produced none of these bytes.
    //
    // Request, netFn 06h rsLUN 2, seq 3Fh rqLUN 1: byte1 = 18h|2 = 1Ah, byte4 = FCh|1 = FDh,
    // chk1 = -(82h+1Ah) = 64h, chk2 = -(20h+FDh+01h) = E2h.
    const LITERAL_REQ_SEQ_3F: [u8; 7] = [0x82, 0x1A, 0x64, 0x20, 0xFD, 0x01, 0xE2];
    // Its response, netFn 07h rqLUN 1, rsLUN 2, data AAh: byte1 = 1Ch|1 = 1Dh, byte4 = FCh|2 = FEh,
    // chk1 = -(20h+1Dh) = C3h, chk2 = -(82h+FEh+01h+00h+AAh) = D5h.
    const LITERAL_RSP_SEQ_3F: [u8; 9] = [0x20, 0x1D, 0xC3, 0x82, 0xFE, 0x01, 0x00, 0xAA, 0xD5];
    // Request, netFn 0Ah rsLUN 3, seq 20h rqLUN 2, from 10h: byte1 = 28h|3 = 2Bh, byte4 = 80h|2 = 82h,
    // chk1 = -(82h+2Bh) = 53h, chk2 = -(10h+82h+10h+00h) = 5Eh.
    const LITERAL_REQ_SEQ_20: [u8; 8] = [0x82, 0x2B, 0x53, 0x10, 0x82, 0x10, 0x00, 0x5E];

    fn assert_re_encodes(frame: &[u8]) {
        let parsed = parse(frame).unwrap();
        let mut out = [0u8; IPMB_MAX_LEN];
        let n = match parsed.completion_code {
            Some(code) => encode_response(&parsed.header, code, parsed.data, &mut out),
            None => encode_request(&parsed.header, parsed.data, &mut out),
        }
        .unwrap();
        assert_eq!(&out[..n], frame);
    }

    #[test]
    fn literal_frames_with_luns_and_high_sequence_bits() {
        let req = parse(&LITERAL_REQ_SEQ_3F).unwrap();
        let h = req.header;
        assert_eq!((h.dest, h.netfn, h.dest_lun), (0x82, 0x06, 2));
        assert_eq!((h.src, h.seq, h.src_lun, h.cmd), (0x20, 0x3F, 1, 0x01));
        assert_eq!((req.completion_code, req.data), (None, &[][..]));
        assert_re_encodes(&LITERAL_REQ_SEQ_3F);

        let rsp = parse(&LITERAL_RSP_SEQ_3F).unwrap();
        let h = rsp.header;
        assert_eq!((h.dest, h.netfn, h.dest_lun), (0x20, 0x07, 1));
        assert_eq!((h.src, h.seq, h.src_lun, h.cmd), (0x82, 0x3F, 2, 0x01));
        assert_eq!((rsp.completion_code, rsp.data), (Some(0), &[0xAA][..]));
        assert_re_encodes(&LITERAL_RSP_SEQ_3F);
        // The generated response header is the field-for-field swap of the request.
        assert_eq!(response_header_for(&req).unwrap(), h);

        let req = parse(&LITERAL_REQ_SEQ_20).unwrap();
        let h = req.header;
        assert_eq!((h.dest, h.netfn, h.dest_lun), (0x82, 0x0A, 3));
        assert_eq!((h.src, h.seq, h.src_lun, h.cmd), (0x10, 0x20, 2, 0x10));
        assert_eq!(req.data, &[0x00]);
        assert_re_encodes(&LITERAL_REQ_SEQ_20);
    }

    #[test]
    fn odd_address_bytes_are_rejected() {
        // Odd destination: 83h with a valid chk1 (-(83h+18h) = 65h).
        assert_eq!(
            parse(&[0x83, 0x18, 0x65, 0x20, 0x14, 0x01, 0xCB]),
            Err(IpmbError::InvalidAddress)
        );
        // Odd source: 21h; chk2 = -(21h+14h+01h) = CAh.
        assert_eq!(
            parse(&[0x82, 0x18, 0x66, 0x21, 0x14, 0x01, 0xCA]),
            Err(IpmbError::InvalidAddress)
        );
        let mut out = [0u8; IPMB_MAX_LEN];
        for bad in [
            Header {
                dest: 0x83,
                ..REQ_HEADER
            },
            Header {
                src: 0x21,
                ..REQ_HEADER
            },
        ] {
            assert_eq!(
                encode_request(&bad, &[], &mut out),
                Err(IpmbError::InvalidAddress)
            );
        }
    }

    #[test]
    fn error_names_are_distinct() {
        let all = [
            IpmbError::TooShort,
            IpmbError::TooLong,
            IpmbError::HeaderChecksum,
            IpmbError::DataChecksum,
            IpmbError::OutBufferTooSmall,
            IpmbError::NotARequest,
            IpmbError::NotAResponse,
            IpmbError::FieldRange,
            IpmbError::InvalidAddress,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.name(), b.name());
            }
        }
    }
}
