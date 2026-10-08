// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The FILE service: the Chaosnet file protocol, as the band's file
//! server spoke it.
//!
//! **The specification is in the release**, and an earlier note here said
//! it was not. `sys/doc/chfile.text`, 792 lines, "Description of the CHAOS
//! FILE protocol designed by HIC": the control connection to contact
//! `FILE`, the `tid <sp> [fh] <sp> cmd [args]` command form, the newline as
//! `NL = 215`, the opcodes `%CODAT = 200` for ASCII, `300` for binary,
//! `201` and `202` for the synchronous and asynchronous marks, `%COEOF =
//! 014`, the error form and its code table. The manual points at `SYS:
//! DOC; FILE TEXT` and the release ships it as `CHFILE TEXT`, which is why
//! it was looked for and not found. It is byte for byte the same file in
//! System 304, two independently restored releases.
//!
//! **And the release carries MIT's own server**, `sys/file/server.lisp`,
//! 1,253 lines: `(chaos:listen "FILE")` and a dispatch of LOGIN, OPEN,
//! OPEN-FOR-LISPM, DATA-CONNECTION, CLOSE, FILEPOS, DELETE, RENAME,
//! EXPUNGE, COMPLETE, CONTINUE, DIRECTORY, CHANGE-PROPERTIES,
//! CREATE-DIRECTORY and CREATE-LINK. So there are three ends to read
//! against each other, not two, and the third is MIT's own.
//!
//! What this is read from, in the order the project ranks them: MIT's
//! specification above, MIT's server, the Lisp Machine's client
//! `sys/network/chaos/qfile.lisp`, and last the MIT/Symbolics Unix server
//! of 1984, `FILE.c`.
//!
//! **The old question about the host had no answer because it had a false
//! premise.** It asked whether the machine at Chaosnet address 3060 ran
//! `FILE.c`, and took `sys/man/pathnm.text`'s "where OZ is a TOPS-20"
//! against `sys/site/hosts.text`'s `HOST MIT-OZ, CHAOS 3060,SERVER,UNIX,
//! VAX,[OZ]` as the release contradicting itself. It does not. The
//! release's own `README`, under "Network Changes", says "] OZ is not
//! really MIT-OZ --- MIT-OZ is identifies now a Unix machine (instead of
//! TOPS-20), and has a new Chaosnet address." The manual describes MIT's
//! historical machine and the host table describes the restoration's
//! stand-in, which are different machines, and 3060 is the restorers'
//! address. No MIT machine was ever there. `sys/site/site.lisp` confirms
//! what the band expects of it: `OZ-SYS-PATHNAME-TRANSLATIONS` is Unix,
//! `("SYS" "//TREE//SYS//")`, which is the tree `src/main.rs` serves.
//!
//! The shape: the user end opens a *control connection* to contact `FILE
//! 1` and sends commands on it as data packets of text, one command a
//! packet, `tid handle COMMAND args`, lines separated by the Lisp
//! Machine's newline. The server answers each with `tid handle COMMAND
//! results`, or `tid handle ERROR code severity message`. Files move on
//! *data connections* the user end listens for and the server calls: a
//! `DATA-CONNECTION` command names an input and an output handle, the
//! server opens a connection to the output handle's name as a contact,
//! and from then on the input handle is that connection's server-to-user
//! direction. `OPEN READ` on an input handle streams the file down it as
//! data packets, then EOF; `CLOSE` is answered on the control connection
//! and followed by a *synchronous mark* on the data connection, which is
//! what the user end reads until.

use super::server::{Out, Response, Service, Session};
use muir::chaos::packet::MAX_DATA;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A counter for temporary-file names, taken once per write and never
/// repeated in this process. Two control connections from one client
/// writing in one directory used to make the same `#muir-…#` name --- the
/// count was kept per control connection --- so opening the second
/// truncated the first's temporary; this is shared across them all.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// The contact name. The band asks for `FILE 1`, the `1` being the
/// protocol version, which arrives as the RFC's argument.
pub const CONTACT: &str = "FILE";
/// The Lisp Machine's newline, `#/NEWLINE`, which is what separates the
/// lines of a command and a reply: `CHNL` in `FILE.c`, `0200|'\r'`.
pub const NEWLINE: u8 = 0o215;
/// Data packet opcodes on a data connection, `qfile.lisp`: character
/// data is plain `DAT`, binary `DAT + 100`, a synchronous mark `DAT + 1`,
/// an asynchronous mark, meaning an error, `DAT + 2`.
pub const CHARACTER_OP: u8 = 0o200;
pub const BINARY_OP: u8 = 0o300;
pub const SYNC_MARK_OP: u8 = 0o201;
pub const ASYNC_MARK_OP: u8 = 0o202;
/// The first two 16-bit words of a compiled file, `QFASL` in sixbit:
/// `FILE.c`'s `QBIN1` 0143150 and `QBIN2` 071660, each low byte first ---
/// `68 c6 b0 73`, which is what `sys/sys/cadrlp.qfasl` in the release
/// begins with.
pub const QFASL_MAGIC: [u8; 4] = [0o150, 0o306, 0o260, 0o163];

/// The refusal every pathname the service may not reach gets: `ATD`,
/// "Access to directory denied".
fn denied() -> (&'static str, String) {
    ("ATD", "Access to directory denied".into())
}

/// The service: files under `root`, served as the server's `/`.
pub struct File {
    root: PathBuf,
    /// A fixed universal time to date things by, or the machine's clock.
    time: Option<u32>,
    /// The Chaosnet addresses this answers, or none for everyone.
    hosts: Option<Vec<u16>>,
}

impl File {
    /// A service answering **every** host that can reach it, which is
    /// what a cable in one process carries: this machine. A CHUDP link
    /// is what puts anyone else on that cable, and
    /// [`super::ChaosServer`] narrows the service with
    /// [`File::serving`] before there is one.
    pub fn new(root: impl Into<PathBuf>) -> File {
        File { root: root.into(), time: None, hosts: None }
    }

    /// Dates by `universal` instead of the machine's clock, if given.
    pub fn with_time(mut self, universal: Option<u32>) -> File {
        self.time = universal;
        self
    }

    /// Answers these Chaosnet addresses and refuses the rest.
    ///
    /// The service reads, writes, renames and deletes a real directory
    /// under containment rules written for a cable with one trusted
    /// machine on it, so who may use it is said in as many words rather
    /// than following from who could send a packet.
    pub fn serving(mut self, hosts: Vec<u16>) -> File {
        self.hosts = Some(hosts);
        self
    }
}

/// The date now --- `time` if fixed, else the machine's clock --- in the
/// same form as a file's.
fn now_date(time: Option<u32>) -> String {
    let secs = match time {
        Some(t) => (t as u64).saturating_sub(super::time::UNIX_EPOCH_UNIVERSAL),
        // The time-neutral harness's date, fixed, for runs that repeat.
        None if super::time::neutral() => super::time::FIXED_UNIX,
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{m:02}/{d:02}/{:02} {hh:02}:{mm:02}:{ss:02}", y % 100)
}

impl Service for File {
    fn contact(&self) -> &str {
        CONTACT
    }
    fn request(&mut self, _now: u64, args: &str, from: (u16, u16)) -> Response {
        // Who may have files is not who could reach the cable: a peer
        // over CHUDP is answerable without being authorised.
        if self.hosts.as_ref().is_some_and(|h| !h.contains(&from.0)) {
            return Response::Refuse(format!("{:o} is not served files by this host", from.0));
        }
        let version = args.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(1);
        Response::Accept(Box::new(Control::new(self.root.clone(), self.time, from.0, version)))
    }
}

/// A data connection as the control connection sees it: the channel
/// behind a pair of file handles. Both ends of one connection share it,
/// so what the user end sends up reaches the control connection's
/// transfer, and what a transfer produces goes down.
#[derive(Default)]
struct Channel {
    open: bool,
    closed: bool,
    /// What is to go down it, in order.
    out: VecDeque<Out>,
    /// What has come up it and not been taken yet: opcode and bytes.
    incoming: VecDeque<(u8, Vec<u8>)>,
    /// Whether the user end has sent its end-of-data.
    eof: bool,
}

/// The data connection's own end: shares the channel with the control
/// connection that asked for it.
struct DataSession(Arc<Mutex<Channel>>);

impl Session for DataSession {
    fn opened(&mut self, _now: u64) {
        self.0.lock().unwrap().open = true;
    }
    fn data(&mut self, _now: u64, op: u8, bytes: &[u8]) {
        self.0.lock().unwrap().incoming.push_back((op, bytes.to_vec()));
    }
    fn eof(&mut self, _now: u64) {
        self.0.lock().unwrap().eof = true;
    }
    fn closed(&mut self, _now: u64, _reason: &str) {
        self.0.lock().unwrap().closed = true;
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        self.0.lock().unwrap().out.drain(..).collect()
    }
}

/// A `DATA-CONNECTION` waiting for its connection to open before it is
/// answered, as `FILE.c` answers it only once `chopen` has succeeded.
struct Pending {
    tid: String,
    channel: Arc<Mutex<Channel>>,
}

/// The control connection.
struct Control {
    root: PathBuf,
    /// A fixed universal time to date things by, or the machine's clock.
    time: Option<u32>,
    client: u16,
    /// The protocol version from the RFC's argument: `FILE 1` is 1. It
    /// chooses the shape of the reply to a write's `CLOSE` --- `FILE.c`
    /// writes the plain form when `protocol > 0` and one with a leading
    /// `-1` for an older client.
    version: u32,
    user: Option<String>,
    /// Handles by name, each to its channel; the input and output
    /// handles of one data connection share a channel.
    ///
    /// Ordered, and not for the order's own sake: the poll walks these,
    /// and with a hashed map that walk is in a different order in every
    /// process, so anything that depends on which handle comes first
    /// happens in some runs and not others. One such bug took an
    /// afternoon to catch. Ordering it does more than take the variation
    /// away: it makes the order that used to lose data the only order
    /// there is, so a test can hold it and fails every time rather than
    /// half of them.
    handles: BTreeMap<String, Arc<Mutex<Channel>>>,
    /// What each handle has open, for CLOSE: a data connection can be
    /// reading on its input handle and writing on its output handle at
    /// the same time.
    transfers: BTreeMap<String, Transfer>,
    pending: Vec<Pending>,
    out: VecDeque<Out>,
}

enum Transfer {
    /// A file being read: its truename and the properties line, repeated
    /// in the CLOSE reply.
    Read {
        truename: String,
        properties: String,
        /// The file as it goes down the wire, kept so that a `FILEPOS`
        /// can send it again from somewhere else, and the opcode it goes
        /// under.
        contents: Vec<u8>,
        op: u8,
    },
    Directory,
    /// A file being written: the temporary it is going into, where it
    /// will be renamed to on close, and how to translate what arrives.
    Write {
        temp: PathBuf,
        real: PathBuf,
        truename: String,
        characters: bool,
        /// Bytes a failed append is holding, waiting for a `CONTINUE`,
        /// and the message that went out in the asynchronous mark. While
        /// this is set the transfer is stopped.
        stalled: Option<(Vec<u8>, String)>,
        /// Whether the synchronous mark that says the data is all there
        /// has come up the data connection. A flag rather than a count
        /// because a write's only inbound mark is that one: the others
        /// the protocol has are the ones a `FILEPOS` or a
        /// `SET-BYTE-SIZE` provokes, and both of those come *from* the
        /// server.
        marked: bool,
        /// The transaction id of a `CLOSE` that came before the mark and
        /// is waiting for it.
        closing: Option<String>,
    },
}

/// What an `OPEN WRITE` was asked for: its option words, the pathname
/// as written and as resolved, and the two that decide the character
/// translation.
struct WriteOpen<'a> {
    words: &'a [&'a str],
    pathname: &'a str,
    path: PathBuf,
    binary: bool,
    default: bool,
}

/// A parsed command: `tid handle COMMAND args`, then the further lines.
struct Command<'a> {
    tid: &'a str,
    handle: &'a str,
    name: &'a str,
    args: &'a str,
    lines: Vec<&'a str>,
}

fn parse(text: &str) -> Option<Command<'_>> {
    let mut lines = text.split(NEWLINE as char);
    let first = lines.next()?;
    let (tid, rest) = first.split_once(' ')?;
    // Two spaces where there is no handle.
    let (handle, rest) = match rest.strip_prefix(' ') {
        Some(r) => ("", r),
        None => rest.split_once(' ')?,
    };
    let (name, args) = match rest.split_once(' ') {
        Some((n, a)) => (n, a),
        None => (rest, ""),
    };
    Some(Command { tid, handle, name, args, lines: lines.collect() })
}

impl Control {
    fn new(root: PathBuf, time: Option<u32>, client: u16, version: u32) -> Control {
        Control {
            time,
            root,
            client,
            version,
            user: None,
            handles: BTreeMap::new(),
            transfers: BTreeMap::new(),
            pending: Vec::new(),
            out: VecDeque::new(),
        }
    }

    /// `tid handle COMMAND results`, `FILE.c`'s `respond`.
    fn reply(&mut self, tid: &str, handle: &str, name: &str, results: &str) {
        let mut line = format!("{tid} {handle} {name}");
        if !results.is_empty() {
            line.push(' ');
            line.push_str(results);
        }
        self.say(&line);
    }

    /// A line down the control connection, in as many packets as it
    /// takes: the connection is a stream, and a reply quoting a command
    /// back --- an unknown command's name, a `LOGIN`'s user in its home
    /// directory --- may run past the [`MAX_DATA`] bytes a packet carries.
    fn say(&mut self, line: &str) {
        for chunk in lispm_text(line).chunks(MAX_DATA) {
            self.out.push_back(Out::Data(chunk.to_vec()));
        }
    }

    /// `tid handle ERROR code severity message`, `FILE.c`'s `error`: the
    /// severity `C` for an error in the command, `F` fatal to a transfer,
    /// `R` recoverable.
    fn error(&mut self, tid: &str, handle: &str, code: &str, severity: char, message: &str) {
        let line = format!("{tid} {handle} ERROR {code} {severity} {message}");
        self.say(&line);
    }

    /// The file under the root that a pathname names, or why not.
    ///
    /// The root is the service's `/`, and nothing the service does may
    /// reach a file outside the tree it serves: [`super::ChaosServer::serving`]
    /// promises that what it writes, renames and deletes stays there. The
    /// tree is the root and what the root's own links lead to ---
    /// the release's fetch script puts its sources under the root by such
    /// a link, `tree` for System 100, under the name that band asks for,
    /// read and written as the band would --- and nothing
    /// else. So the pathname is taken component by component under the
    /// root, with `..` and `.` refused rather than followed; then the
    /// deepest part of the result that exists is resolved on the host,
    /// links and all, and must lie under the root or under what one of the
    /// root's own entries links to. A link anywhere deeper that leads out
    /// is refused, and a link the band makes with `CREATE-LINK` can only
    /// point under the root, so the band cannot widen the tree. A refusal
    /// is `ATD`, "Access to directory denied", the error `FILE.c` gives a
    /// pathname the user may not reach.
    fn resolve(&self, pathname: &str) -> Result<PathBuf, (&'static str, String)> {
        let mut p = self.root.clone();
        for part in pathname.split('/').filter(|s| !s.is_empty()) {
            if part == ".." || part == "." {
                return Err(denied());
            }
            p.push(part);
        }
        let mut tree = vec![std::fs::canonicalize(&self.root).map_err(|_| denied())?];
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for e in entries.flatten() {
                if e.file_type().is_ok_and(|t| t.is_symlink())
                    && let Ok(real) = std::fs::canonicalize(e.path())
                {
                    tree.push(real);
                }
            }
        }
        let mut existing = p.as_path();
        let real = loop {
            match std::fs::canonicalize(existing) {
                Ok(real) => break real,
                Err(_) => existing = existing.parent().ok_or_else(denied)?,
            }
        };
        if tree.iter().any(|t| real.starts_with(t)) { Ok(p) } else { Err(denied()) }
    }

    /// [`Self::resolve`] for a pathname that is to be written, renamed,
    /// deleted or created: the root itself is none of those.
    fn resolve_for_writing(&self, pathname: &str) -> Result<PathBuf, (&'static str, String)> {
        let p = self.resolve(pathname)?;
        if p == self.root { Err(denied()) } else { Ok(p) }
    }

    fn command(&mut self, text: &[u8]) {
        let text = from_bytes(text);
        let Some(c) = parse(&text) else {
            return;
        };
        let (tid, handle) = (c.tid.to_string(), c.handle.to_string());
        match c.name {
            "LOGIN" => {
                let user = c.args.split_whitespace().next().unwrap_or("").to_string();
                if user.is_empty() {
                    self.error(&tid, &handle, "UNK", 'C', "Unknown user");
                    return;
                }
                // `FILE.c`: the name, the home directory with a slash, the
                // full name; the user end takes the home directory and the
                // personal name off the two lines.
                let home = format!("/{}/", user.to_lowercase());
                let results = format!("{user} {home}{}{user}{}", NEWLINE as char, NEWLINE as char);
                self.user = Some(user);
                self.reply(&tid, &handle, "LOGIN", &results);
            }
            "DATA-CONNECTION" => {
                let mut a = c.args.split_whitespace();
                let (Some(input), Some(output)) = (a.next(), a.next()) else {
                    self.error(&tid, &handle, "BUG", 'C', "DATA-CONNECTION wants two handles");
                    return;
                };
                if self.handles.contains_key(input) || self.handles.contains_key(output) {
                    self.error(&tid, &handle, "BUG", 'C', "File handle already exists");
                    return;
                }
                let channel = Arc::new(Mutex::new(Channel::default()));
                self.handles.insert(input.to_string(), channel.clone());
                self.handles.insert(output.to_string(), channel.clone());
                // "The output file handle name is the contact name the user
                // end is listening for, so send it."
                self.out.push_back(Out::Connect {
                    host: self.client,
                    contact: output.to_string(),
                    session: Box::new(DataSession(channel.clone())),
                });
                self.pending.push(Pending { tid, channel });
            }
            "UNDATA-CONNECTION" => {
                // Both handles of the data connection go, and the
                // transfers on both of them: "UNDATA-CONNECTION implies
                // a CLOSE on each file handle of the DATA connection for
                // which there is a file transfer in progress". The
                // client names the input handle --- `qfile.lisp` sends
                // `(DATA-INPUT-HANDLE DATA-CONN)` --- and a write is on
                // the output one, so taking only the named handle's
                // transfer would leave a write behind, and its temporary
                // in the directory.
                let mut going = vec![handle.clone()];
                if let Some(ch) = self.handles.remove(&handle) {
                    ch.lock().unwrap().out.push_back(Out::Close("Undata".into()));
                    going.extend(
                        self.handles
                            .iter()
                            .filter(|(_, v)| Arc::ptr_eq(v, &ch))
                            .map(|(k, _)| k.clone()),
                    );
                    self.handles.retain(|_, v| !Arc::ptr_eq(v, &ch));
                }
                for h in going {
                    if let Some(Transfer::Write { temp, closing, .. }) = self.transfers.remove(&h) {
                        let _ = std::fs::remove_file(&temp);
                        self.stranded(&h, closing);
                    }
                }
                self.reply(&tid, &handle, "UNDATA-CONNECTION", "");
            }
            "OPEN" => self.open(&tid, &handle, c.args, c.lines.first().copied().unwrap_or("")),
            "DIRECTORY" => {
                self.directory(&tid, &handle, c.args, c.lines.first().copied().unwrap_or(""))
            }
            "CLOSE" => self.close(&tid, &handle),
            "DELETE" => self.delete(&tid, &handle, c.lines.first().copied().unwrap_or("")),
            "RENAME" => self.rename(
                &tid,
                &handle,
                c.lines.first().copied().unwrap_or(""),
                c.lines.get(1).copied().unwrap_or(""),
            ),
            "CREATE-DIRECTORY" => {
                self.create_directory(&tid, &handle, c.lines.first().copied().unwrap_or(""))
            }
            "CREATE-LINK" => self.create_link(
                &tid,
                &handle,
                c.lines.first().copied().unwrap_or(""),
                c.lines.get(1).copied().unwrap_or(""),
            ),
            // `FILE.c`'s `expunge` answers with the number of blocks it
            // recovered, and on Unix, where a delete is a delete, that is
            // always none.
            "EXPUNGE" => self.reply(&tid, &handle, "EXPUNGE", "0"),
            "CHANGE-PROPERTIES" => self.change_properties(&tid, &handle, &c.lines),
            "COMPLETE" => self.complete(
                &tid,
                &handle,
                c.args,
                c.lines.first().copied().unwrap_or(""),
                c.lines.get(1).copied().unwrap_or(""),
            ),
            // Position within a transfer: nothing here reads a file in
            // pieces, so the only position that can be asked for is the
            // one it is already at.
            "FILEPOS" => self.filepos(&tid, &handle, c.args.trim()),
            "PROPERTIES" => self.properties(&tid, &handle, c.lines.first().copied().unwrap_or("")),
            // `CONTINUE` resumes a transfer the server stopped with a
            // recoverable error, and the client only ever sends it after
            // an **asynchronous mark**: `QFILE-PROCESS-ASYNC-MARK` puts
            // the stream in `:ASYNC-MARKED`, and `:CONTINUE`'s own guard
            // is `(EQ STATUS :ASYNC-MARKED)`, so without a mark it does
            // nothing. The one mark sent here is a write that ran out of
            // room, `NMR` from `wrote`, so that is the one
            // transfer that can be continued; any other handle gets what
            // `FILE.c` answers, "CONTINUE received when not in error
            // state".
            "CONTINUE" => self.cont(&tid, &handle),
            other => {
                self.error(&tid, &handle, "UKC", 'C', &format!("{other} is not served here"));
            }
        }
    }

    /// `OPEN direction mode options`, then the pathname on the next line.
    fn open(&mut self, tid: &str, handle: &str, args: &str, pathname: &str) {
        let words: Vec<&str> = args.split_whitespace().collect();
        let direction = words.first().copied().unwrap_or("READ");
        let binary = words.contains(&"BINARY");
        let default = words.contains(&"DEFAULT");
        let given_byte_size = words
            .iter()
            .position(|w| *w == "BYTE-SIZE")
            .and_then(|i| words.get(i + 1))
            .and_then(|n| n.parse::<u32>().ok());
        let resolved = if direction == "WRITE" {
            self.resolve_for_writing(pathname)
        } else {
            self.resolve(pathname)
        };
        let path = match resolved {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if direction == "WRITE" {
            return self.open_write(
                tid,
                handle,
                WriteOpen { words: &words, pathname, path, binary, default },
            );
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        };
        if direction == "PROBE-DIRECTORY" || (direction == "PROBE" && meta.is_dir()) {
            let results = format!(
                "{} 0 NIL{}{}{}",
                date(&meta),
                NEWLINE as char,
                truename(pathname, true),
                NEWLINE as char
            );
            self.reply(tid, handle, "OPEN", &results);
            return;
        }
        if meta.is_dir() {
            return self.error(tid, handle, "FNF", 'C', "That is a directory");
        }
        // Only a regular file is read. A FIFO with no writer blocks the
        // read for ever, a device streams without end: either would hold
        // or exhaust the engine's thread, so what is neither a directory
        // nor a regular file is refused with `WKF`, the band's
        // `WRONG-KIND-OF-FILE` in `sys/io/file/open.lisp`. PROBE reaches
        // this too, reading the whole file to measure it.
        if !meta.is_file() {
            return self.error(tid, handle, "WKF", 'C', "Not a regular file");
        }
        let Ok(contents) = std::fs::read(&path) else {
            return self.error(tid, handle, "ATF", 'C', "Access to file denied");
        };
        let qfasl = contents.starts_with(&QFASL_MAGIC);
        // Characters unless the file is compiled, when DEFAULT asks.
        // `FILE.c` decides that first and only then has a byte size:
        // `options |= O_CHARACTER` happens inside the DEFAULT case, and
        // the length is `options & O_CHARACTER || bytesize <= 8 ?
        // st_size : (st_size + 1) / 2` --- bytes for characters, words
        // for binary, so a compiled file opened DEFAULT is measured in
        // words even though nobody said BINARY.
        let characters = if default { !qfasl } else { !binary };
        let byte_size = given_byte_size.unwrap_or(if characters { 8 } else { 16 });
        let length =
            if characters || byte_size <= 8 { contents.len() } else { contents.len().div_ceil(2) };
        // `FILE.c`: date, length, QFASL as T or NIL, and with DEFAULT the
        // characters decision as T or NIL after a space.
        let mut properties =
            format!("{} {} {}", date(&meta), length, if qfasl { "T" } else { "NIL" });
        if default {
            properties.push_str(if characters { " T" } else { " NIL" });
        }
        let tn = truename(pathname, false);
        let results = format!("{properties}{}{tn}{}", NEWLINE as char, NEWLINE as char);
        if direction == "PROBE" {
            self.reply(tid, handle, "OPEN", &results);
            return;
        }
        // READ: down the input handle's data connection.
        let Some(channel) = self.handles.get(handle).cloned() else {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        };
        self.reply(tid, handle, "OPEN", &results);
        let (op, bytes) =
            if characters { (CHARACTER_OP, to_lispm(&contents)) } else { (BINARY_OP, contents) };
        {
            let mut ch = channel.lock().unwrap();
            for chunk in bytes.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(op, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
        self.transfers.insert(
            handle.to_string(),
            Transfer::Read { truename: tn, properties, contents: bytes, op },
        );
    }

    /// `DIRECTORY options`, the pathname on the next line: the listing
    /// goes down the input handle as records of text, `FILE.c`'s
    /// `diropen` and `dirread`: a first record with the file system's
    /// properties, then one a file, each a pathname line, property lines
    /// `NAME value`, and a blank line.
    fn directory(&mut self, tid: &str, handle: &str, _args: &str, pathname: &str) {
        let Some(channel) = self.handles.get(handle).cloned() else {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        };
        let (dir, pattern) = match pathname.rsplit_once('/') {
            Some((d, p)) => (d.to_string(), p.to_string()),
            None => (String::new(), pathname.to_string()),
        };
        let dir_path = match self.resolve(&dir) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let mut names: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir_path) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if matches(&pattern, &name) {
                    names.push(name);
                }
            }
        } else {
            return self.error(tid, handle, "DNF", 'C', "Directory not found");
        }
        names.sort();
        let nl = NEWLINE as char;
        let mut text = String::new();
        text.push(nl);
        text.push_str(&format!("BLOCK-SIZE 1024{nl}"));
        text.push_str(&format!("SETTABLE-PROPERTIES CREATION-DATE AUTHOR{nl}"));
        text.push(nl);
        for name in names {
            let full = dir_path.join(&name);
            let Ok(meta) = std::fs::metadata(&full) else { continue };
            let shown = format!("{}/{}", dir.trim_end_matches('/'), name);
            text.push_str(&format!("{shown}{nl}"));
            text.push_str(&format!("AUTHOR {}{nl}", self.user.as_deref().unwrap_or("nobody")));
            text.push_str(&format!("BYTE-SIZE 8{nl}"));
            text.push_str(&format!("LENGTH-IN-BLOCKS {}{nl}", meta.len().div_ceil(1024)));
            text.push_str(&format!("LENGTH-IN-BYTES {}{nl}", meta.len()));
            text.push_str(&format!("CREATION-DATE {}{nl}", date(&meta)));
            if meta.is_dir() {
                text.push_str(&format!("DIRECTORY T{nl}"));
            }
            text.push(nl);
        }
        self.reply(tid, handle, "DIRECTORY", "");
        {
            // A byte a character: `text` holds the protocol's newline at
            // 0o215, which `as_bytes` would encode as two.
            let bytes = lispm_text(&text);
            let mut ch = channel.lock().unwrap();
            for chunk in bytes.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(CHARACTER_OP, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
        self.transfers.insert(handle.to_string(), Transfer::Directory);
    }

    /// `CLOSE`: answered on the control connection, and for a read
    /// followed by the synchronous mark down the data connection, in
    /// that order --- "we must respond to the close before sending the
    /// SYNCMARK since otherwise we would likely block".
    ///
    /// For a write it is the other way about: the mark is **awaited**,
    /// and only then does the close rename the temporary into place and
    /// answer with the file's date, its length and its truename ---
    /// `FILE.c`'s `xclose`, which writes that plain form when the
    /// protocol version is above zero and one with a leading `-1` for an
    /// older client.
    fn close(&mut self, tid: &str, handle: &str) {
        // Any data that arrived on the data connection but has not yet
        // been moved into the write goes in before the temporary is
        // renamed: a CLOSE can follow the last data packet with no turn
        // of the server between them.
        self.drain_incoming(handle);
        // A write's CLOSE comes up the control connection and its mark
        // up the data connection, and the two can overtake each other:
        // `sys/doc/chfile.text`'s worked example for writing a file
        // sends "a SYNC mark on the DATA connection and a CLOSE on the
        // CONTROL connection (in either order)", so a CLOSE that
        // arrives first is a correct client's doing rather than a
        // fault. The mark is what says the data is all there, and
        // renaming without it would put a file into place short of
        // whatever had not arrived --- empty, if none of it had. So the
        // transfer stays open and the CLOSE is answered from the poll
        // once the mark has come.
        //
        // The client sends the two that way round and does not wait
        // between them: `qfile.lisp`'s `:COMMAND` writes the command
        // packet on the control connection and then, for an output
        // stream, `(SEND STREAM :WRITE-SYNCHRONOUS-MARK)` before it
        // waits for the response. So holding the reply back cannot
        // hold the mark back with it.
        if let Some(Transfer::Write { marked: false, stalled: None, closing, .. }) =
            self.transfers.get_mut(handle)
        {
            *closing = Some(tid.to_string());
            return;
        }
        let Some(transfer) = self.transfers.remove(handle) else {
            return self.error(
                tid,
                handle,
                "BUG",
                'C',
                "No transfer in progress on this file handle",
            );
        };
        match transfer {
            Transfer::Read { truename, properties, .. } => {
                let results =
                    format!("{properties}{}{truename}{}", NEWLINE as char, NEWLINE as char);
                self.reply(tid, handle, "CLOSE", &results);
                if let Some(ch) = self.handles.get(handle) {
                    ch.lock().unwrap().out.push_back(Out::DataOp(SYNC_MARK_OP, Vec::new()));
                }
            }
            Transfer::Directory => {
                self.reply(tid, handle, "CLOSE", "");
                if let Some(ch) = self.handles.get(handle) {
                    ch.lock().unwrap().out.push_back(Out::DataOp(SYNC_MARK_OP, Vec::new()));
                }
            }
            Transfer::Write { stalled: Some((_, why)), temp, .. } => {
                // The file would be short of whatever the stall is
                // holding, so it does not go into place; the error that
                // stopped it is repeated, fatal this time.
                let _ = std::fs::remove_file(&temp);
                self.error(tid, handle, "NMR", 'F', &why);
            }
            Transfer::Write { temp, real, truename, .. } => {
                if let Err(e) = std::fs::rename(&temp, &real) {
                    let _ = std::fs::remove_file(&temp);
                    return self.error(tid, handle, "MSC", 'F', &e.to_string());
                }
                let length = std::fs::metadata(&real).map(|m| m.len()).unwrap_or(0);
                let when = std::fs::metadata(&real)
                    .map(|m| date(&m))
                    .unwrap_or_else(|_| now_date(self.time));
                let nl = NEWLINE as char;
                let body = format!("{when} {length}{nl}{truename}{nl}");
                let results = if self.version > 0 { body } else { format!("-1 {body}") };
                self.reply(tid, handle, "CLOSE", &results);
            }
        }
    }

    /// Answers a `CLOSE` that was still waiting for its synchronous mark
    /// when the write it was waiting on was taken away --- an
    /// `UNDATA-CONNECTION` or a `DELETE` on the same handle. `CNO`,
    /// "CLOSE on non-open channel", is `chfile.text`'s code for it: by
    /// the time the CLOSE could be answered there was no longer a
    /// channel to close.
    ///
    /// The band never gets here. Its `:REAL-CLOSE` waits for the CLOSE's
    /// reply before it frees the data connection, its abort route sends
    /// the DELETE first --- when the CLOSE that follows finds no
    /// transfer at all --- and it only undoes a data connection that has
    /// gone dormant. This is so that a client which does it the other
    /// way round is told, rather than left waiting for a reply that
    /// would never come.
    fn stranded(&mut self, handle: &str, closing: Option<String>) {
        if let Some(tid) = closing {
            self.error(&tid, handle, "CNO", 'C', "The transfer was abandoned before its mark");
        }
    }

    /// `OPEN WRITE`, then the pathname: a temporary file beside the real
    /// one, which `CLOSE` renames into place --- `FILE.c` creates
    /// `tempfile(dirname)` and links it over the real name on close,
    /// "we know that both names are in the same directory".
    ///
    /// `IF-EXISTS` and `IF-DOES-NOT-EXIST` say what to do about what is
    /// there; the client sends them by name. `NEW-VERSION` is what a
    /// versioned file system does and this one has no versions, so it is
    /// `SUPERSEDE` here, which is what `FILE.c` turns it into.
    fn open_write(&mut self, tid: &str, handle: &str, o: WriteOpen<'_>) {
        let WriteOpen { words, pathname, path, binary, default } = o;
        let named = |key: &str| {
            words.iter().position(|w| *w == key).and_then(|i| words.get(i + 1)).copied()
        };
        let if_exists = named("IF-EXISTS").unwrap_or("NEW-VERSION");
        let if_missing = named("IF-DOES-NOT-EXIST").unwrap_or("CREATE");
        let exists = path.exists();
        if exists {
            match if_exists {
                "ERROR" => {
                    return self.error(tid, handle, "FAE", 'C', "File already exists");
                }
                "NEW-VERSION" | "SUPERSEDE" | "RENAME" | "RENAME-AND-DELETE" | "TRUNCATE"
                | "OVERWRITE" | "APPEND" => {}
                other => {
                    return self.error(
                        tid,
                        handle,
                        "UOO",
                        'C',
                        &format!("{other} is not a way to write an existing file"),
                    );
                }
            }
        } else if if_missing == "ERROR" {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        }
        let Some(dir) = path.parent() else {
            return self.error(tid, handle, "DNF", 'C', "Directory not found");
        };
        if !dir.is_dir() {
            return self.error(tid, handle, "DNF", 'C', "Directory not found");
        }
        if !self.handles.contains_key(handle) {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        }
        // A FIFO or a device where the file would be is not a regular
        // file to overwrite; reading it for APPEND or OVERWRITE would
        // block or exhaust the engine's thread, so it is refused with
        // `WKF`, the band's `WRONG-KIND-OF-FILE` in
        // `sys/io/file/open.lisp`, before a temporary is made.
        if exists && !std::fs::metadata(&path).map(|m| m.is_file()).unwrap_or(false) {
            return self.error(tid, handle, "WKF", 'C', "Not a regular file");
        }
        // The name is taken from a process-wide counter so that two
        // control connections writing in one directory never collide.
        let temp = dir.join(format!(
            "#muir-{}-{}#",
            self.client,
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        // What is already there, for APPEND, and to start from for
        // OVERWRITE; otherwise the temporary starts empty.
        let start = match if_exists {
            "APPEND" | "OVERWRITE" if exists => std::fs::read(&path).unwrap_or_default(),
            _ => Vec::new(),
        };
        if std::fs::write(&temp, &start).is_err() {
            return self.error(tid, handle, "ATD", 'C', "Access to directory denied");
        }
        let characters = if default { true } else { !binary };
        let tn = truename(pathname, false);
        // The reply is the same shape as a read's: the file's date, its
        // length, whether it is compiled. Nothing is written yet.
        let results = format!(
            "{} {} NIL{}{tn}{}",
            now_date(self.time),
            start.len(),
            NEWLINE as char,
            NEWLINE as char
        );
        self.reply(tid, handle, "OPEN", &results);
        self.transfers.insert(
            handle.to_string(),
            Transfer::Write {
                temp,
                real: path,
                truename: tn,
                characters,
                stalled: None,
                marked: false,
                closing: None,
            },
        );
    }

    /// Takes everything waiting on a handle's data connection and gives it
    /// to the write in progress on that handle, in order. A handle with no
    /// transfer has its waiting data discarded --- there is nowhere for it
    /// to go, and keeping it would grow without bound.
    fn drain_incoming(&mut self, handle: &str) {
        let Some(ch) = self.handles.get(handle) else {
            return;
        };
        let items: Vec<(u8, Vec<u8>)> = ch.lock().unwrap().incoming.drain(..).collect();
        let writing = self.transfers.contains_key(handle);
        for (op, bytes) in items {
            // A synchronous mark says the data is all there; it carries
            // none of its own, and it is what a CLOSE waits for.
            if op == SYNC_MARK_OP {
                if let Some(Transfer::Write { marked, .. }) = self.transfers.get_mut(handle) {
                    *marked = true;
                }
                continue;
            }
            if op == ASYNC_MARK_OP {
                continue;
            }
            if writing {
                self.wrote(handle, op, &bytes);
            }
        }
    }

    /// Data that has come up a data connection: appended to whatever the
    /// handle is writing, translated out of the Lisp Machine character
    /// set if it is characters.
    fn wrote(&mut self, handle: &str, op: u8, bytes: &[u8]) {
        let Some(Transfer::Write { temp, characters, stalled, .. }) =
            self.transfers.get_mut(handle)
        else {
            return;
        };
        let out = if *characters && op != BINARY_OP { from_lispm(bytes) } else { bytes.to_vec() };
        // Already stopped: the client should have stopped sending, but
        // whatever arrives joins what is waiting rather than being lost.
        if let Some((held, _)) = stalled.as_mut() {
            held.extend_from_slice(&out);
            return;
        }
        let temp = temp.clone();
        if let Err(e) = append(&temp, &out) {
            let why = e.to_string();
            if let Some(Transfer::Write { stalled, .. }) = self.transfers.get_mut(handle) {
                *stalled = Some((out, why.clone()));
            }
            self.async_mark(handle, "NMR", &why);
        }
    }

    /// Stops a transfer with a **recoverable** error, AIM-628's
    /// `E_RECOVERABLE`: an asynchronous mark down the data connection,
    /// which `FILE.c`'s `fherror` writes as `TIDNO <handle> ERROR <code>
    /// R <message>`, the literal `TIDNO` standing where a transaction id
    /// would be. The client's `QFILE-PROCESS-ASYNC-MARK` strips that
    /// first word, shows the error as proceedable, and sends `CONTINUE`
    /// if the user proceeds.
    fn async_mark(&mut self, handle: &str, code: &str, message: &str) {
        let line = format!("TIDNO {handle} ERROR {code} R {message}");
        if let Some(ch) = self.handles.get(handle) {
            ch.lock().unwrap().out.push_back(Out::DataOp(ASYNC_MARK_OP, lispm_text(&line)));
        }
    }

    /// `CONTINUE`: retries what the stalled append is holding.
    ///
    /// `FILE.c` answers the command and lets the transfer retry after ---
    /// `filecontinue` sets `X_RETRY` and responds --- so the reply says
    /// only that the command was understood, and a retry that fails
    /// again raises another mark. With no transfer, or one that is not
    /// stopped, it is the server's own `BUG`, "CONTINUE received when
    /// not in error state".
    fn cont(&mut self, tid: &str, handle: &str) {
        let stopped =
            matches!(self.transfers.get(handle), Some(Transfer::Write { stalled: Some(_), .. }));
        if handle.is_empty() || !self.transfers.contains_key(handle) {
            return self.error(tid, handle, "BUG", 'C', "No transfer to continue");
        }
        if !stopped {
            return self.error(
                tid,
                handle,
                "BUG",
                'C',
                "CONTINUE received when not in error state",
            );
        }
        self.reply(tid, handle, "CONTINUE", "");
        let Some(Transfer::Write { temp, stalled, .. }) = self.transfers.get_mut(handle) else {
            return;
        };
        let (held, _) = stalled.take().expect("stopped");
        let temp = temp.clone();
        if let Err(e) = append(&temp, &held) {
            let why = e.to_string();
            if let Some(Transfer::Write { stalled, .. }) = self.transfers.get_mut(handle) {
                *stalled = Some((held, why.clone()));
            }
            self.async_mark(handle, "NMR", &why);
        }
    }

    /// `DELETE`: on a handle in the middle of a write it abandons the
    /// temporary and leaves the real file alone, which is what the
    /// client's `:REAL-CLOSE` does when it aborts; with a pathname and no
    /// handle it removes the file.
    fn delete(&mut self, tid: &str, handle: &str, pathname: &str) {
        if !handle.is_empty()
            && let Some(Transfer::Write { temp, closing, .. }) = self.transfers.remove(handle)
        {
            let _ = std::fs::remove_file(&temp);
            self.stranded(handle, closing);
            return self.reply(tid, handle, "DELETE", "");
        }
        let path = match self.resolve_for_writing(pathname) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        };
        let gone =
            if meta.is_dir() { std::fs::remove_dir(&path) } else { std::fs::remove_file(&path) };
        match gone {
            Ok(()) => self.reply(tid, handle, "DELETE", ""),
            Err(e) if meta.is_dir() => {
                self.error(tid, handle, "DNE", 'C', &format!("Directory not empty: {e}"))
            }
            Err(e) => self.error(tid, handle, "ATF", 'C', &e.to_string()),
        }
    }

    /// `RENAME`, the old pathname then the new.
    fn rename(&mut self, tid: &str, handle: &str, old: &str, new: &str) {
        let (from, to) = match (self.resolve_for_writing(old), self.resolve_for_writing(new)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err((code, msg)), _) | (_, Err((code, msg))) => {
                return self.error(tid, handle, code, 'C', &msg);
            }
        };
        if !from.exists() {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        }
        if to.exists() {
            return self.error(tid, handle, "REF", 'C', "Rename to existing file");
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => self.reply(tid, handle, "RENAME", ""),
            Err(e) => self.error(tid, handle, "ATF", 'C', &e.to_string()),
        }
    }

    /// `CREATE-DIRECTORY`, the pathname on the next line.
    fn create_directory(&mut self, tid: &str, handle: &str, pathname: &str) {
        let path = match self.resolve_for_writing(pathname) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if path.exists() {
            return self.error(tid, handle, "DAE", 'C', "Directory already exists");
        }
        match std::fs::create_dir(&path) {
            Ok(()) => self.reply(tid, handle, "CREATE-DIRECTORY", ""),
            Err(e) => self.error(tid, handle, "CCD", 'C', &e.to_string()),
        }
    }

    /// `CREATE-LINK`, the link then what it points at.
    fn create_link(&mut self, tid: &str, handle: &str, link: &str, target: &str) {
        let (link_path, target_path) = match (self.resolve_for_writing(link), self.resolve(target))
        {
            (Ok(a), Ok(b)) => (a, b),
            (Err((code, msg)), _) | (_, Err((code, msg))) => {
                return self.error(tid, handle, code, 'C', &msg);
            }
        };
        if link_path.exists() {
            return self.error(tid, handle, "FAE", 'C', "File already exists");
        }
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target_path, &link_path);
        #[cfg(not(unix))]
        let made: std::io::Result<()> =
            Err(std::io::Error::other("links are not made on this platform"));
        match made {
            Ok(()) => self.reply(tid, handle, "CREATE-LINK", ""),
            Err(e) => self.error(tid, handle, "CCL", 'C', &e.to_string()),
        }
    }

    /// `CHANGE-PROPERTIES`, the pathname then `NAME value` a line. Only
    /// the ones a file here has are settable; the rest are refused by
    /// name, as `FILE.c` refuses what its property table has no setter
    /// for.
    fn change_properties(&mut self, tid: &str, handle: &str, lines: &[&str]) {
        let pathname = lines.first().copied().unwrap_or("");
        let path = match self.resolve_for_writing(pathname) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if !path.exists() {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        }
        for line in lines.iter().skip(1).filter(|l| !l.is_empty()) {
            let name = line.split(' ').next().unwrap_or("");
            match name {
                // The dates and the author are what `FILE.c` can set;
                // nothing here keeps an author, and a date is the file's
                // own, which is left as the filesystem has it.
                "CREATION-DATE" | "MODIFICATION-DATE" | "REFERENCE-DATE" | "AUTHOR" => {}
                "" => {}
                other => {
                    return self.error(
                        tid,
                        handle,
                        "UKP",
                        'C',
                        &format!("{other} cannot be set here"),
                    );
                }
            }
        }
        self.reply(tid, handle, "CHANGE-PROPERTIES", "");
    }

    /// `COMPLETE options`, the default pathname then the string to
    /// complete. The reply is a status word and the completion, a line
    /// each: the client reads the word as a keyword and takes `NIL` for
    /// no completion.
    fn complete(&mut self, tid: &str, handle: &str, args: &str, default: &str, partial: &str) {
        let new_ok = args.contains("NEW-OK");
        let (dir, stem) = match partial.rsplit_once('/') {
            Some((d, p)) => (d.to_string(), p.to_string()),
            None => (
                default.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(),
                partial.to_string(),
            ),
        };
        let dir_path = match self.resolve(&dir) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let mut hits: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir_path) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&stem) {
                    hits.push(name);
                }
            }
        }
        hits.sort();
        let nl = NEWLINE as char;
        let (status, completed) = match hits.len() {
            0 if new_ok => ("NEW", format!("{}/{stem}", dir.trim_end_matches('/'))),
            0 => ("NIL", format!("{}/{stem}", dir.trim_end_matches('/'))),
            1 => ("OLD", format!("{}/{}", dir.trim_end_matches('/'), hits[0])),
            _ => {
                // The longest head they share, which is as far as a
                // completion can go.
                let mut common = hits[0].clone();
                for h in &hits[1..] {
                    // By character and not by byte: a name is UTF-8 on the
                    // host, and a cut inside a character is not a string.
                    let n = common
                        .chars()
                        .zip(h.chars())
                        .take_while(|(a, b)| a == b)
                        .map(|(a, _)| a.len_utf8())
                        .sum();
                    common.truncate(n);
                }
                ("NIL", format!("{}/{common}", dir.trim_end_matches('/')))
            }
        };
        self.reply(tid, handle, "COMPLETE", &format!("{status}{nl}{completed}{nl}"));
    }

    /// `FILEPOS <n>`: the read goes on from byte `n`.
    ///
    /// The client sends this with its mark flag set --- `:COMMAND T
    /// "File Position" "FILEPOS " n` --- and then reads until a
    /// synchronous mark, which is how it throws away what is already in
    /// flight from the old position. So the reply comes first, then the
    /// mark, and then the file again from where it was asked for.
    ///
    /// The position is in the bytes as they go down the wire, which for
    /// a character file is after the translation: that is what the
    /// client counts, having read them itself.
    fn filepos(&mut self, tid: &str, handle: &str, arg: &str) {
        let Ok(at) = arg.parse::<usize>() else {
            return self.error(tid, handle, "FOR", 'C', "Filepos out of range");
        };
        let Some(Transfer::Read { contents, op, .. }) = self.transfers.get(handle) else {
            return self.error(tid, handle, "BUG", 'C', "No transfer to position");
        };
        if at > contents.len() {
            return self.error(tid, handle, "FOR", 'C', "Filepos out of range");
        }
        let (rest, op) = (contents[at..].to_vec(), *op);
        self.reply(tid, handle, "FILEPOS", "");
        if let Some(ch) = self.handles.get(handle) {
            let mut ch = ch.lock().unwrap();
            // What is still queued from the old position never goes.
            ch.out.retain(|o| !matches!(o, Out::DataOp(..) | Out::Eof));
            ch.out.push_back(Out::DataOp(SYNC_MARK_OP, Vec::new()));
            for chunk in rest.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(op, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
    }

    /// `PROPERTIES`, the pathname on the next line: one record down the
    /// data connection, the same shape a directory's entries have.
    fn properties(&mut self, tid: &str, handle: &str, pathname: &str) {
        let Some(channel) = self.handles.get(handle).cloned() else {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        };
        let path = match self.resolve(pathname) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        };
        let nl = NEWLINE as char;
        let mut text = format!("{}{nl}", truename(pathname, meta.is_dir()));
        text.push_str(&self.file_properties(&meta));
        text.push(nl);
        self.reply(tid, handle, "PROPERTIES", "");
        {
            let bytes = lispm_text(&text);
            let mut ch = channel.lock().unwrap();
            for chunk in bytes.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(CHARACTER_OP, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
        self.transfers.insert(handle.to_string(), Transfer::Directory);
    }

    /// The property lines a file has, `NAME value` each, from `FILE.c`'s
    /// property table.
    fn file_properties(&self, meta: &std::fs::Metadata) -> String {
        let nl = NEWLINE as char;
        let mut t = String::new();
        t.push_str(&format!("AUTHOR {}{nl}", self.user.as_deref().unwrap_or("nobody")));
        t.push_str(&format!("BYTE-SIZE 8{nl}"));
        t.push_str(&format!("LENGTH-IN-BLOCKS {}{nl}", meta.len().div_ceil(1024)));
        t.push_str(&format!("LENGTH-IN-BYTES {}{nl}", meta.len()));
        t.push_str(&format!("CREATION-DATE {}{nl}", date(meta)));
        if meta.is_dir() {
            t.push_str(&format!("DIRECTORY T{nl}"));
        }
        t
    }
}

impl Session for Control {
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.command(bytes);
    }
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {
        // A write that never reached its CLOSE --- the user end's
        // connection dropped, or the machine rebooted under it --- leaves
        // a temporary that will never be renamed into place; remove it.
        for transfer in self.transfers.values() {
            if let Transfer::Write { temp, .. } = transfer {
                let _ = std::fs::remove_file(temp);
            }
        }
        for ch in self.handles.values() {
            ch.lock().unwrap().out.push_back(Out::Close("Control connection closed".into()));
        }
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        // Data connections that have opened since: answered now.
        let mut opened = Vec::new();
        self.pending.retain(|p| {
            if p.channel.lock().unwrap().open {
                opened.push(p.tid.clone());
                false
            } else {
                true
            }
        });
        for tid in opened {
            self.reply(&tid, "", "DATA-CONNECTION", "");
        }
        // What the user end has sent up a data connection belongs to the
        // **write** in progress on that connection, and to nothing else.
        // Both handles of a connection share one channel, so draining the
        // handle that is writing takes it all; the other is left alone,
        // or it would steal its sibling's data --- and it will have a
        // transfer of its own whenever the user end is reading a file and
        // writing one at once, which is what the band does through every
        // compile (`sys/qcfile.lisp`'s `QC-FILE` holds the source open
        // around the QFASL it writes). Selecting on *a* transfer rather
        // than a write is how a compiled file came to be written to no
        // one: the read's drain took the write's bytes, dropped them for
        // having nowhere to go, and the clear below finished the job.
        // `tests/chaos.rs::a_read_and_a_write_on_one_data_connection_keep_their_own_bytes`.
        let writing: Vec<String> = self
            .handles
            .keys()
            .filter(|h| matches!(self.transfers.get(*h), Some(Transfer::Write { .. })))
            .cloned()
            .collect();
        for handle in writing {
            self.drain_incoming(&handle);
        }
        // A CLOSE that overtook its synchronous mark waits in the
        // transfer; the mark has now come, so it can be answered. A
        // write that has stalled since goes through here too, to the
        // error the stalled arm gives it: a stall holds bytes that have
        // not been written, so no mark can make that file whole.
        let ready: Vec<(String, String)> = self
            .transfers
            .iter()
            .filter_map(|(handle, transfer)| match transfer {
                Transfer::Write { closing: Some(tid), marked, stalled, .. }
                    if *marked || stalled.is_some() =>
                {
                    Some((tid.clone(), handle.clone()))
                }
                _ => None,
            })
            .collect();
        for (tid, handle) in ready {
            self.close(&tid, &handle);
        }
        // Anything still waiting has no write to go to --- data before an
        // OPEN WRITE, or after a CLOSE --- and is discarded, so a channel
        // never grows without bound.
        for ch in self.handles.values() {
            ch.lock().unwrap().incoming.clear();
        }
        self.out.drain(..).collect()
    }
}

/// The name as the user end will see it: the pathname it asked for.
fn truename(pathname: &str, directory: bool) -> String {
    let t = pathname.trim_end_matches('/');
    if directory { format!("{t}/") } else { t.to_string() }
}

/// `MM/DD/YY HH:MM:SS`, the form `PARSE-DIRECTORY-DATE-PROPERTY` reads
/// fastest, from the file's modification time, in UTC.
fn date(meta: &std::fs::Metadata) -> String {
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Under the time-neutral harness every file has its fixed date: the
    // run's own files are made when it runs.
    let secs = if super::time::neutral() { super::time::FIXED_UNIX } else { secs };
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{m:02}/{d:02}/{:02} {hh:02}:{mm:02}:{ss:02}", y % 100)
}

/// Calendar date and time from seconds since 1970, UTC: Howard Hinnant's
/// days-to-civil.
pub fn civil(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m as u64, d as u64, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Adds bytes to the end of a file that must already be there.
fn append(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.write_all(bytes)
}

/// Lisp Machine text back to Unix, `FILE.c`'s `from_lispm`: the inverse
/// of [`to_lispm`], and the note beside it in the C is worth keeping ---
/// "0212 maps to 015 since 0215 must map to 012".
pub fn from_lispm(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&c| match c {
            0o10 | 0o11 | 0o12 | 0o14 | 0o15 | 0o177 => c | 0o200,
            0o212 => 0o15,
            NEWLINE => b'\n',
            0o210 | 0o211 | 0o214 | 0o377 => c & 0o177,
            _ => c,
        })
        .collect()
}

/// Unix text to the Lisp Machine character set, `FILE.c`'s `to_lispm`:
/// newline to the Lisp Machine's, the format effectors 010 to 015 and
/// 0177 up into the 0200s where the Lisp Machine keeps them, and the
/// bytes 0210 to 0215 and 0377 --- a Lisp Machine file's format
/// effectors as Unix stored them --- back down.
pub fn to_lispm(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&c| match c {
            0o210 | 0o211 | 0o212 | 0o214 | 0o215 | 0o377 => c & 0o177,
            b'\n' => NEWLINE,
            0o15 => 0o212,
            0o10 | 0o11 | 0o14 | 0o177 => c | 0o200,
            _ => c,
        })
        .collect()
}

/// Protocol text as bytes: each character is one byte, so the Lisp
/// Machine's newline at 0o215 survives.
fn lispm_text(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect()
}

/// The bytes of a command as one character each, the inverse of
/// [`lispm_text`].
///
/// **Not UTF-8.** The protocol's own newline is 0o215, which is a
/// continuation byte in UTF-8, so reading a command with
/// `String::from_utf8_lossy` replaces it and every line after the first
/// is lost --- an `OPEN` then reads its pathname as empty and answers
/// about the root directory. `the_file_service_serves_files_and_directories`
/// in `tests/chaos.rs` is the regression.
fn from_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// `*` any run, `?` any one, else itself: the glob the user end sends.
///
/// The two-pointer match, linear in the lengths: walk the name, and on a
/// mismatch fall back to the last `*` seen, advancing by one the name
/// position it is taken to have matched to. The recursive form that tried
/// both branches of every `*` was exponential in the number of stars ---
/// a pattern of ten `*a` against a long run of `a`s pinned the engine's
/// thread for billions of tries. Bytes, not characters, as the rest of
/// the matcher is: a name is UTF-8 on the host, and `?` matching one byte
/// is what it did before.
pub fn matches(pattern: &str, name: &str) -> bool {
    // An empty pattern matches anything, as it did before: DIRECTORY of a
    // path ending in a slash lists the whole directory.
    if pattern.is_empty() {
        return true;
    }
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    let (mut pi, mut ni) = (0, 0);
    // The last `*` in the pattern, and the name position it is currently
    // taken to have matched up to; `None` until one is seen.
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            // Back to the last `*`, letting it swallow one more character.
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, ni));
        } else {
            return false;
        }
    }
    // The name is used up; any trailing `*`s match the empty run.
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}
