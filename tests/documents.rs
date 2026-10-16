// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The numbers the documents quote about the netlists.
//!
//! `data/README.md` carries a table of the netlists: the pages, the `part`
//! records and how many parts are on each board. `docs/netlists.md` says
//! the same totals in prose.
//! **Nothing read any of them.** `tests/parts_mounted.rs` asserts the same
//! figures against the netlists and says in a comment that the documents
//! quote them, but it restates the constants rather than reading them, so
//! the netlists were held to the numbers and the documents to nobody.
//!
//! That fails worse than a document nothing checks at all. Change a
//! netlist, `parts_mounted` fails, somebody updates the constant, and both
//! documents are wrong from that moment **with a green suite saying
//! otherwise**: a second copy that nothing reconciles is worse than no
//! copy, because it looks checked.
//!
//! So the numbers are read out of the documents here and compared with the
//! netlists, which are the thing they are about. Both are committed, so
//! this never skips.

use std::collections::BTreeMap;

use muir::netlist::{self, Netlist};

mod support;

/// The netlists, by the name their row gives: `data/`'s own files, read
/// from disk rather than listed, so that a ninth is covered when it lands.
fn netlists() -> BTreeMap<String, String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).expect("data/ is committed") {
        let p = e.expect("a directory entry").path();
        if p.extension().is_some_and(|x| x == "netlist") {
            let name = p.file_name().expect("a file").to_string_lossy().into_owned();
            out.insert(name, std::fs::read_to_string(&p).expect("a netlist"));
        }
    }
    assert!(out.len() >= 8, "data/ holds the netlists");
    out
}

/// A table read out of a document, as rows of cells keyed by the header.
///
/// **Found by its header rather than by where it is**, so that a document
/// can be rearranged, a column can move, and the padding of a Markdown
/// table can be redone by anything that formats one, without this needing
/// to know. What it cannot survive is a column being *renamed*, which is
/// the right thing to fail on: the claim being made here is about that
/// column.
struct Table {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// The cell in `row` under the column called `name`.
    fn get<'a>(&self, row: &'a [String], name: &str) -> &'a str {
        let k = self.columns.iter().position(|c| c == name);
        let k = k.unwrap_or_else(|| panic!("no column {name:?} in {:?}", self.columns));
        &row[k]
    }

    /// That cell as a number, with the thousands separator a document may
    /// write and a table of numbers usually does not.
    fn number(&self, row: &[String], name: &str) -> usize {
        let cell = self.get(row, name).replace(',', "");
        cell.parse().unwrap_or_else(|_| panic!("{name} is not a number: {cell:?}"))
    }
}

/// The Markdown table whose header row names every one of `columns`.
fn markdown(text: &str, columns: &[&str]) -> Table {
    let cells = |line: &str| -> Vec<String> {
        line.trim()
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().trim_matches('`').to_string())
            .collect()
    };
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim_start().starts_with('|') {
            continue;
        }
        let head = cells(line);
        if !columns.iter().all(|c| head.iter().any(|h| h == c)) {
            continue;
        }
        // The `|---|` rule under the header, then the rows.
        lines.next();
        let mut rows = Vec::new();
        while lines.peek().is_some_and(|l| l.trim_start().starts_with('|')) {
            rows.push(cells(lines.next().expect("peeked")));
        }
        return Table { columns: head, rows };
    }
    panic!("no Markdown table with the columns {columns:?}");
}

/// How many devices are mounted on the board a netlist describes, which is
/// what both documents mean by a part. `tests/parts_mounted.rs` is the
/// account of the three numbers a netlist can be counted by, and the
/// counting itself is `support::parts_on`, shared with it rather than
/// written again here.
///
/// **`CADR.netlist` is two boards in one file and they share designators**,
/// so counting the whole of it as one set of locations merges them: 691
/// where the machine has 985. The control store's pages are counted apart
/// from the processor's, which is what makes the sum right for that file
/// and changes nothing for the seven that are one board each.
fn parts(text: &str) -> usize {
    let n: Netlist = netlist::parse(text).expect("a netlist parses");
    let icmem = support::control_store_pages();
    let on = |store: bool| support::parts_on(&n, |page| icmem.contains(page) == store).len();
    on(false) + on(true)
}

/// **`data/README.md` has a row for every file in `data/`**, which is
/// `CLAUDE.md`'s rule for that directory, and the pages, records and parts
/// in each row are the netlist's.
#[test]
fn the_data_inventory_counts_every_netlist_correctly() {
    const README: &str = include_str!("../data/README.md");
    let t = markdown(README, &["File", "Pages", "Records", "Parts"]);
    let mut seen = 0;
    for netlist in t.rows.iter() {
        let file = t.get(netlist, "File").to_string();
        let Some(text) = netlists().get(&file).cloned() else { continue };
        seen += 1;
        let pages = text.lines().filter(|l| l.starts_with("page ")).count();
        let records = text.lines().filter(|l| l.starts_with("part ")).count();
        assert_eq!(t.number(netlist, "Pages"), pages, "{file}: pages");
        assert_eq!(t.number(netlist, "Records"), records, "{file}: part records");
        assert_eq!(t.number(netlist, "Parts"), parts(&text), "{file}: parts mounted");
    }
    assert_eq!(seen, netlists().len(), "a row for every netlist in data/");
}

/// **The part counts `docs/netlists.md` quotes are the netlists'.** The
/// whole machine's total is quoted in prose rather than in a table, so it
/// is read out of the sentence that makes the claim and summed here from
/// the same netlists.
#[test]
fn the_netlists_page_quotes_the_netlists_own_part_counts() {
    const NETLISTS: &str = include_str!("../docs/netlists.md");
    let files = netlists();

    // A whole machine: the processor pair, the bus interface, one memory
    // board, the I/O board, the disk controller and both display boards ---
    // a SIMPLE TV as the main display and a LISPM TV beside it as the color
    // TV. The disk multiplexor is not in it, being fitted only when a run
    // asks for it.
    let board = |file: &str| parts(files.get(file).unwrap_or_else(|| panic!("no data/{file}")));
    let memory = board("CADRM.netlist");
    let processor = board("CADR.netlist");
    let machine = processor
        + board("BUSINT.netlist")
        + memory
        + board("CADRIO.netlist")
        + board("CADRDC.netlist")
        + board("SIMPLETV.netlist")
        + board("LISPMTV.netlist");
    let full = machine + 31 * memory;
    for (what, text, before, after, want) in [
        (
            "docs/netlists.md",
            NETLISTS,
            "A whole machine is ",
            " parts with one memory board",
            machine,
        ),
        ("docs/netlists.md", NETLISTS, "the color TV, and ", " with all thirty-two", full),
    ] {
        assert_eq!(quoted(what, text, before, after), want, "{what}: {before:?}");
    }
}

/// A document's prose as one flow. The column a Markdown paragraph wraps at
/// is typography and not a claim, so it is collapsed before a sentence is
/// looked for.
fn flowed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The number a document writes between `before` and `after`, with the
/// thousands separator prose uses and a table does not.
///
/// **The words around the figure are as much of the claim as the figure
/// is**: what a number counts is the sentence it sits in, so a document
/// that has stopped saying this has stopped quoting the count, and that
/// fails here exactly as a wrong figure does.
fn quoted(what: &str, text: &str, before: &str, after: &str) -> usize {
    let flow = flowed(text);
    let at = flow.find(before);
    let at = at.unwrap_or_else(|| panic!("{what} does not say {before:?}"));
    assert_eq!(flow.matches(before).count(), 1, "{what} says {before:?} more than once");
    let rest = &flow[at + before.len()..];
    let end = rest.find(after);
    let end = end.unwrap_or_else(|| panic!("{what}: {before:?} is not followed by {after:?}"));
    let cell = rest[..end].replace(',', "");
    cell.parse().unwrap_or_else(|_| panic!("{what}: {cell:?} is not a number"))
}

/// **A count written out in words goes stale in silence**, and this one
/// had: `data/README.md` said "the seven netlists" twice and `README.md`
/// once, after the disk multiplexor made eight. Nothing spelled the number
/// wrong --- the tables were right all along --- so nothing looked.
#[test]
fn the_documents_say_how_many_netlists_there_are_in_words() {
    const WORDS: [&str; 13] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve",
    ];
    let n = netlists().len();
    let right = WORDS.get(n).unwrap_or_else(|| panic!("{n} netlists is past this list"));
    for (what, text) in [
        ("README.md", include_str!("../README.md")),
        ("data/README.md", include_str!("../data/README.md")),
        ("docs/manual.md", include_str!("../docs/manual.md")),
        ("docs/engines.md", include_str!("../docs/engines.md")),
        ("docs/machine.md", include_str!("../docs/machine.md")),
        ("docs/netlists.md", include_str!("../docs/netlists.md")),
        ("docs/sources.md", include_str!("../docs/sources.md")),
    ] {
        for word in WORDS {
            let said = format!("the {word} netlists");
            assert!(
                !text.contains(&said) || word == *right,
                "{what} says {said:?}, and data/ holds {n}"
            );
        }
    }
}

/// **Two `chip` runs took System 100 from the pack to its who-line with
/// every board a netlist**, and three documents quote how long each took:
/// `docs/netlists.md`, `docs/engines.md` and `docs/manual.md`.
///
/// Nothing here can measure a run again --- each is days of gate-level
/// simulation --- so what the figures are held to is each other. That is
/// the failure that would otherwise pass unseen: `docs/netlists.md` and the
/// manual quoting two different boots of the same machine, each looking
/// right where it stands.
#[test]
fn the_documents_agree_on_the_cold_boot_runs() {
    const NETLISTS: &str = include_str!("../docs/netlists.md");
    const ENGINES: &str = include_str!("../docs/engines.md");
    const MANUAL: &str = include_str!("../docs/manual.md");

    /// A run on the wall clock, and the microcycles it executed, in
    /// millions, from the boot PROM's first to `Cold-booted` on the screen.
    struct Run {
        days: usize,
        hours: usize,
        millions: usize,
    }
    /// 12 September 2026: the processor, the bus interface, main memory,
    /// the I/O board, the SIMPLE TV and MIT's disk controller.
    const FIRST: Run = Run { days: 2, hours: 14, millions: 301 };
    /// 15 September 2026: the same, with the DISK MULTIPLEXOR on the
    /// controller's cable as well.
    const SECOND: Run = Run { days: 2, hours: 21, millions: 306 };

    // `docs/engines.md` quotes the wall clock and not the microcycles, so
    // the wall clock is all that is read out of it. `docs/manual.md` quotes
    // the first run twice, and each is read.
    for (what, text, before, after, want) in [
        ("docs/netlists.md", NETLISTS, "It took ", " days", FIRST.days),
        ("docs/netlists.md", NETLISTS, "It took 2 days ", " hours", FIRST.hours),
        ("docs/netlists.md", NETLISTS, "2 days 14 hours and ", " million", FIRST.millions),
        ("docs/netlists.md", NETLISTS, "came up the same way, in ", " days", SECOND.days),
        ("docs/netlists.md", NETLISTS, "same way, in 2 days ", " hours", SECOND.hours),
        ("docs/netlists.md", NETLISTS, "2 days 21 hours and ", " million", SECOND.millions),
        ("docs/manual.md", MANUAL, "this controller on 12 September 2026 in ", " days", FIRST.days),
        (
            "docs/manual.md",
            MANUAL,
            "controller on 12 September 2026 in 2 days ",
            " hours",
            FIRST.hours,
        ),
        ("docs/manual.md", MANUAL, "2026 in 2 days 14 hours and ", " million", FIRST.millions),
        ("docs/manual.md", MANUAL, "this way on 12 September 2026 after ", " days", FIRST.days),
        ("docs/manual.md", MANUAL, "way on 12 September 2026 after 2 days ", " hours", FIRST.hours),
        ("docs/manual.md", MANUAL, "2026 after 2 days 14 hours and ", " million", FIRST.millions),
        ("docs/engines.md", ENGINES, "12 September 2026, after ", " days", FIRST.days),
        ("docs/engines.md", ENGINES, "2026, after 2 days ", " hours", FIRST.hours),
        ("docs/engines.md", ENGINES, "on the cable as well, after ", " days", SECOND.days),
        ("docs/engines.md", ENGINES, "as well, after 2 days ", " hours", SECOND.hours),
    ] {
        assert_eq!(quoted(what, text, before, after), want, "{what}: {before:?}");
    }
}

/// **`docs/chaosnet.md` quotes numbers that live in the source.** The
/// intervals on that page were measured on the netlist board and then
/// frozen into a `const` in `src/chaos/`; the register addresses and CSR
/// masks are AIM-628 §7's, as `src/chaos/interface.rs` holds them. The
/// page is a second copy of all of it, and a second copy that nothing
/// reconciles is worse than no copy at all, because it looks checked.
/// So the page is read here and held to the code it describes.
///
/// **What is not read back, and why.** The page also quotes figures that
/// are not constants anywhere: the 257 slots two of the board's own
/// packets sit apart, the 12,060 and 12,310 ns either side of the busy
/// receiver's abort, and AIM-628's own 4032 data bits and 64-microsecond
/// token. Those are measured inside a test or quoted from the memo rather
/// than stored, so there is nothing here to compare them against --- the
/// tests the page names beside each are what hold them.
#[test]
fn the_chaosnet_page_quotes_the_sources_own_numbers() {
    use muir::chaos::interface::csr;
    use muir::chaos::{board, ether, interface, packet, udp, wire};
    const CHAOSNET: &str = include_str!("../docs/chaosnet.md");

    // The table of measured constants: a row names one and gives the
    // nanoseconds it holds.
    let t = markdown(CHAOSNET, &["Constant", "Nanoseconds"]);
    let want: BTreeMap<&str, u64> = [
        ("CELL_NS", wire::CELL_NS),
        ("SAMPLE_NS", wire::SAMPLE_NS),
        ("LOCKOUT_NS", wire::LOCKOUT_NS),
        ("IDLE_NS", wire::IDLE_NS),
        ("SLOT_NS", ether::SLOT_NS),
        ("ROUND_NS", ether::ROUND_NS),
        ("ABORT_NS", ether::ABORT_NS),
        ("ABORT_HOLD_NS", ether::ABORT_HOLD_NS),
        ("BUSY_ABORT_NS", ether::BUSY_ABORT_NS),
        ("RACT_NS", ether::RACT_NS),
        ("REFILL_WORD_NS", ether::REFILL_WORD_NS),
        ("TURN_TC_NS", board::TURN_TC_NS),
        ("TURN_FIRST_TC_NS", board::TURN_FIRST_TC_NS),
        ("TURN_LOAD_NS", board::TURN_LOAD_NS),
        ("TURN_START_NS", board::TURN_START_NS),
        ("CBLBSY_OFF_NS", board::CBLBSY_OFF_NS),
        ("CBLBSY_HOLD_NS", board::CBLBSY_HOLD_NS),
        ("FCLK_RISE_NS", board::FCLK_RISE_NS),
        ("TDONE_BEFORE_END_NS", board::TDONE_BEFORE_END_NS),
        ("TSR_LOAD_NS", board::TSR_LOAD_NS),
        ("TSR_WORD_NS", board::TSR_WORD_NS),
        ("TSR_READY_NS", board::TSR_READY_NS),
        ("TDONE_AFTER_ABORT_NS", board::TDONE_AFTER_ABORT_NS),
    ]
    .into_iter()
    .collect();
    let mut seen = 0;
    for row in &t.rows {
        let name = t.get(row, "Constant");
        let n = want.get(name);
        let n =
            n.unwrap_or_else(|| panic!("chaosnet.md names a constant nothing here has: {name}"));
        assert_eq!(t.number(row, "Nanoseconds") as u64, *n, "chaosnet.md: {name}");
        seen += 1;
    }
    assert_eq!(seen, want.len(), "a row for every constant the page is held to");

    /// An octal figure as the page writes it, MIT's own way of writing
    /// these: a base the table cannot be read in without knowing it.
    fn octal(what: &str, cell: &str) -> u32 {
        u32::from_str_radix(cell, 8)
            .unwrap_or_else(|_| panic!("chaosnet.md: {what} {cell:?} is not octal"))
    }

    // The registers, by the name AIM-628 §7 gives each.
    let t = markdown(CHAOSNET, &["Address", "Register", "Read or write"]);
    let registers: BTreeMap<&str, u32> = [
        ("Command/Status Register", interface::CSR),
        ("My Address", interface::MY_ADDRESS),
        ("Write Buffer", interface::WRITE_BUFFER),
        ("Read Buffer", interface::READ_BUFFER),
        ("Bit Count", interface::BIT_COUNT),
        ("Start Transmission", interface::START),
    ]
    .into_iter()
    .collect();
    let mut seen = 0;
    for row in &t.rows {
        let name = t.get(row, "Register");
        let at = registers.get(name);
        let at = at.unwrap_or_else(|| panic!("chaosnet.md names a register nothing has: {name}"));
        assert_eq!(octal("the address", t.get(row, "Address")), *at, "chaosnet.md: {name}");
        seen += 1;
    }
    assert_eq!(seen, registers.len(), "a row for every register");

    // The CSR's bits, by the name AIM-628 §7 gives each.
    let t = markdown(CHAOSNET, &["Mask", "Bit", "Kind"]);
    let bits: BTreeMap<&str, u16> = [
        ("Timer Interrupt Enable", csr::TIMER_INT_ENABLE),
        ("Loop Back", csr::LOOP_BACK),
        ("Spy", csr::SPY),
        ("Clear Receiver", csr::CLEAR_RECEIVER),
        ("Receive Interrupt Enable", csr::RECEIVE_INT_ENABLE),
        ("Transmit Interrupt Enable", csr::TRANSMIT_INT_ENABLE),
        ("Transmit Abort", csr::TRANSMIT_ABORT),
        ("Transmit Done", csr::TRANSMIT_DONE),
        ("Clear Transmitter", csr::CLEAR_TRANSMITTER),
        ("Lost Count", csr::LOST_COUNT),
        ("Reset", csr::RESET),
        ("CRC Error", csr::CRC_ERROR),
        ("Receive Done", csr::RECEIVE_DONE),
    ]
    .into_iter()
    .collect();
    let mut seen = 0;
    for row in &t.rows {
        let name = t.get(row, "Bit");
        let mask = bits.get(name);
        let mask =
            mask.unwrap_or_else(|| panic!("chaosnet.md names a CSR bit nothing has: {name}"));
        assert_eq!(octal("the mask", t.get(row, "Mask")) as u16, *mask, "chaosnet.md: {name}");
        seen += 1;
    }
    assert_eq!(seen, bits.len(), "a row for every CSR bit");

    // **Lost Count's width is the claim, not its mask's value**: the page
    // calls the field four bits, so the mask must have four.
    assert_eq!(csr::LOST_COUNT.count_ones(), 4, "Lost Count is four bits wide");
    assert!(
        flowed(CHAOSNET).contains("four bits holding"),
        "chaosnet.md should still call Lost Count four bits"
    );

    // The figures the prose states rather than tabulates.
    for (before, after, want) in [
        ("one slot is ", " ns and a whole round is", ether::SLOT_NS as usize),
        ("a whole round is ", " of them", (ether::ROUND_NS / ether::SLOT_NS) as usize),
        ("assigns as ", " over MIT's", udp::TRANSMIT_TRIES as usize),
        ("bits --- the ", " bytes §3.5 gives", packet::MAX_DATA),
    ] {
        assert_eq!(quoted("docs/chaosnet.md", CHAOSNET, before, after), want, "{before:?}");
    }

    // The interrupt vector, which the page quotes from AIM-628 in octal.
    assert_eq!(board::VECTOR, 0o270);
    assert!(
        flowed(CHAOSNET).contains("Chaosnet interface is 270"),
        "chaosnet.md should quote AIM-628's interrupt vector"
    );
}
