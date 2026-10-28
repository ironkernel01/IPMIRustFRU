# ipmc-log

The capture log and the logging logic of the DEBUG menu, in one crate: the record ring
(`record.rs`), the CSV and Wireshark-style printers (`print.rs`), the paced dump (`dump.rs`), the
trace tee (`tee.rs`) and the log switches with their status lines (`switches.rs`). `no_std`, no
hardware, no allocator. It depends on `ipmc-core`; the core never depends on it.

The firmware glue (the console handlers DEBUG LOG, DUMP, CLEAR and TRACE) is
`crates/ipmc-runtime/src/log_cmds.rs`. `tools/capture/capture2pcap.py` turns a DEBUG DUMP
CSV into a Wireshark pcap.
