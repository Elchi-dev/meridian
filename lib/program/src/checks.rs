//! Checks of every call from ring 3, for the QEMU test image, where the
//! test build of hello runs them. Each logs "ok" or "FAIL" with its name,
//! and hello ends with the number that failed, which the kernel's check
//! wants to be 0.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use abi::{LOG_BYTES, MESSAGE_BYTES};

use security::{Integrity, Privileges, Restriction, Token};

use crate::{
    Ended, Error, Handle, Info, channel, close, duplicate, info, key, list, log, make_directory,
    map, open, open_with, raw, read, receive, remove, rename, send, sleep, spawn, spawn_handing,
    spawn_restricted, spawn_with, start, stream, terminate, token, unmap, wait, write, write_file,
};

type Outcome = Result<(), &'static str>;

type Check = fn() -> Outcome;

const CHECKS: [(&str, Check); 171] = [
    ("log", lines),
    ("log.format", formatted),
    ("ticks", time_goes_on),
    ("stack", the_stack_asked_for),
    ("channel", round_trip),
    ("channel.empty", nothing_waits),
    ("channel.room", room_too_small),
    ("channel.longest", longest_message),
    ("channel.carry", handle_carried),
    ("channel.closed", other_end_closed),
    ("channel.full", full),
    ("close", closed_twice),
    ("handle", not_a_handle),
    ("memory", pages_come_and_go),
    ("memory.sizes", sizes_there_are),
    ("alloc", heap_in_use),
    ("spawn", child_exits_with_what_it_was_sent),
    ("spawn.fault", child_runs_out_of_stack),
    ("spawn.missing", no_such_program),
    ("terminate", child_ended_from_outside),
    ("thread", threads_of_a_child),
    ("thread.here", a_thread_of_its_own),
    ("thread.moves", a_thread_takes_what_it_was_given),
    ("thread.stack", a_thread_has_all_its_stack),
    ("wait.other", wait_for_no_process),
    ("token", own_token),
    ("spawn.token", child_gets_no_more),
    ("file", settings_read_back),
    ("file.refused", files_that_do_not_open),
    ("file.list", directories_list_their_entries),
    ("file.write", files_written),
    ("file.write_refused", writes_refused),
    ("file.rename", files_renamed),
    ("file.source", sources_close_their_files),
    ("directory", working_directory),
    ("spawn.file", child_from_its_file),
    ("handle.duplicate", duplicates_stand_for_the_same_object),
    ("spawn.arguments", arguments_arrive),
    ("spawn.output", output_goes_where_it_is_handed),
    ("spawn.input", input_is_read_to_its_end),
    ("spawn.error", errors_go_apart),
    ("stream", streams_end_and_close_what_they_bring),
    ("shell.script", shell_runs_a_script),
    ("shell.builtins", the_shells_own_commands),
    ("shell.status", shell_ends_with_the_status),
    ("shell.deep", functions_call_each_other_deep),
    ("shell.programs", shell_starts_programs),
    ("shell.files", shell_writes_files),
    ("shell.patterns", shell_matches_patterns),
    ("shell.file", shell_runs_a_file),
    ("shell.redirections", shell_redirects),
    ("shell.pipes", shell_pipes),
    ("shell.pipes.trouble", pipes_that_go_wrong),
    ("shell.pipes.files", pipes_and_files),
    ("shell.pipes.long", a_long_pipeline),
    ("shell.input", shell_hands_its_input_on),
    ("shell.input.ended", input_that_ended_stays_ended),
    ("shell.redirections.both", reading_and_writing_files_at_once),
    ("shell.output.file", programs_write_into_the_shells_file),
    ("shell.bytes", bytes_go_through_as_they_are),
    ("shell.carry", files_copied_and_moved),
    ("shell.directory", the_working_directory),
    ("shell.root", the_root_is_a_directory),
    ("shell.errors", errors_go_where_2_sends_them),
    ("shell.errors.output", errors_go_with_the_output),
    ("tools", head_grep_and_sort),
    ("tools.files", tools_on_files),
    ("tools.input", tools_read_the_shells_input),
    ("tools.long", long_lines_and_many),
    ("tools.empty", tools_given_nothing),
    ("tools.tail", tail_and_uniq),
    ("tools.tail.files", tail_and_uniq_on_files),
    ("tools.big", uniq_on_a_big_file),
    ("tools.chain", the_tools_one_after_another),
    ("tools.input.more", tail_and_uniq_read_the_shells_input),
    ("tools.named", files_named_past_one_not_there),
    ("shell.pipes.own", the_shells_own_commands_in_pipes),
    ("shell.carry.itself", no_copy_onto_itself),
    ("shell.carry.capitals", a_move_to_other_capitals),
    ("shell.trouble", the_shells_own_commands_say_what_went_wrong),
    (
        "shell.output.gone",
        the_shell_ends_with_1_when_its_output_is_gone,
    ),
    ("shell.partial", a_character_cut_short_shows),
    ("shell.closes", files_close_when_a_command_cannot_start),
    ("shell.shared", the_shared_commands),
    ("shell.terminal", streams_that_are_terminals),
    ("shell.test.files", test_asks_the_files),
    ("shell.sh", the_sh_dialect),
    ("shell.sh.streams", sh_streams),
    ("shell.sh.streams.errors", sh_errors_follow_stream_2),
    ("shell.sh.here", sh_here_documents),
    ("shell.sh.session", sh_session),
    ("shell.sh.errors", sh_errors),
    ("shell.sh.files", sh_files),
    ("shell.sh.pipes", sh_pipelines),
    ("shell.sh.captures", sh_captures),
    ("shell.sh.functions", sh_function_streams),
    ("shell.sh.parameters", sh_parameters),
    ("shell.sh.file", sh_script_files),
    ("shell.sh.names", sh_names_of_files),
    ("shell.sh.read", sh_read),
    ("shell.sh.trap", sh_trap),
    ("shell.sh.dot", sh_dot),
    ("shell.sh.cd", sh_cd),
    ("shell.sh.command", sh_command),
    ("shell.sh.command.paths", sh_command_paths),
    ("shell.sh.started", sh_started),
    ("shell.sh.exec", sh_exec),
    ("shell.sh.jobs", sh_jobs),
    ("shell.sh.jobs.many", sh_jobs_side_by_side),
    ("shell.sh.exec.files", sh_exec_files),
    ("shell.sh.read.files", sh_read_files),
    ("tools.refused", tools_refuse_what_is_no_text),
    ("tools.grep", grep_expressions),
    ("tools.sed", sed_on_its_input),
    ("tools.sed.files", sed_on_files),
    ("tools.sed.long", sed_on_a_long_text),
    ("tools.sed.program", sed_as_a_program),
    ("tools.sed.wide", sed_on_wide_lines),
    ("tools.sed.pipes", sed_among_the_tools),
    ("tools.expr", expr_on_its_words),
    ("tools.sleep", sleep_waits),
    ("tools.cmp", cmp_on_files),
    ("tools.comm", comm_on_files),
    ("tools.paste", paste_joins_lines),
    ("tools.fold", fold_breaks_lines),
    ("tools.nl", nl_numbers_lines),
    ("tools.od", od_dumps_bytes),
    ("tools.wc", wc_counts_what_it_is_asked),
    ("tools.expand", expand_and_unexpand),
    ("tools.join", join_pairs_lines),
    ("tools.split", split_cuts_files),
    ("tools.split.many", split_makes_many_files),
    ("tools.uniq", uniq_options),
    ("tools.sort", sort_options),
    ("tools.commands", commands_as_programs),
    ("tools.xargs", xargs_runs_programs),
    ("tools.xargs.streams", xargs_hands_its_streams),
    ("tools.cksum", cksum_sums),
    ("tools.tsort", tsort_orders),
    ("tools.diff", diff_on_files),
    ("tools.patch", patch_puts_diffs_in),
    ("tools.find", find_walks_trees),
    ("tools.csplit", csplit_cuts_files),
    ("tools.pathchk", pathchk_checks_names),
    ("tools.dd", dd_copies_blocks),
    ("tools.rm", rm_removes_files),
    ("tools.mkdir", mkdir_makes_directories),
    ("tools.rmdir", rmdir_removes_directories),
    ("tools.cat", cat_writes_files),
    ("tools.awk", awk_runs_programs),
    ("tools.ls", ls_lists_directories),
    ("tools.cp", cp_copies_files),
    ("tools.mv", mv_moves_files),
    ("tools.strings", strings_finds_text),
    ("tools.du", du_counts_blocks),
    ("tools.touch", touch_makes_files),
    ("tools.unlink", unlink_removes_a_file),
    ("tools.ed", ed_edits_files),
    ("tools.uuencode", uuencode_writes_text),
    ("tools.uudecode", uudecode_makes_files),
    ("tools.bc", bc_works_out_numbers),
    ("tools.compress", compress_squeezes_files),
    ("tools.file", file_names_kinds),
    ("tools.names", basename_dirname_and_tr),
    ("tools.tr.long", tr_on_a_long_file),
    ("tools.cut.tee", cut_and_tee),
    ("tools.tee.trouble", tee_where_it_cannot_write),
    ("tools.cut.files", cut_on_files),
    ("tools.tee.anew", tee_writes_anew),
    ("tools.cut.trouble", cut_refuses_and_leaves_out),
    ("console", writes_on_the_console),
];

/// Runs every check and answers how many failed.
pub fn run() -> u64 {
    let mut failed = 0;
    for (name, check) in CHECKS {
        let _ = match check() {
            Ok(()) => crate::log!("ok program.{name}"),
            Err(why) => {
                failed += 1;
                crate::log!("FAIL program.{name}: {why}")
            }
        };
    }
    failed
}

fn expect(ok: bool, why: &'static str) -> Outcome {
    if ok { Ok(()) } else { Err(why) }
}

fn pair() -> Result<(Handle, Handle), &'static str> {
    channel().map_err(|_| "a channel should be made")
}

fn lines() -> Outcome {
    expect(log("a line") == Ok(()), "a short line should be taken")?;
    let long = [b'x'; LOG_BYTES + 1];
    let long = core::str::from_utf8(&long).map_err(|_| "x is text")?;
    expect(
        log(long) == Err(Error::TooLong),
        "a line longer than the log takes should be refused",
    )?;
    expect(
        log(&long[..LOG_BYTES]) == Ok(()),
        "the longest line should be taken",
    )
}

fn formatted() -> Outcome {
    let (two, four) = (2, 4);
    expect(
        crate::log!("{two} and {two} make {four}") == Ok(()),
        "a formatted line should be taken",
    )?;
    let mut line = crate::Line::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{two}{four}"));
    expect(line.as_str() == "24", "a line should hold what was written")
}

/// A program gets the stack its note asks for: hello asks for 32 pages
/// when it runs these checks, and this goes 96 KiB down into them, past
/// the 64 KiB every program gets.
fn the_stack_asked_for() -> Outcome {
    /// Goes down the stack until it stands 96 KiB below `top`, and answers
    /// the sum of what it kept on the way: 0.
    fn down(top: usize) -> u64 {
        let room = core::hint::black_box([0u8; 256]);
        let here = core::ptr::from_ref(&room) as usize;
        let below = if top - here < 96 * 1024 { down(top) } else { 0 };
        below + u64::from(room[here % room.len()])
    }
    let top = 0u8;
    expect(
        down(core::ptr::from_ref(&top) as usize) == 0,
        "a program should get the stack its note asks for",
    )
}

fn time_goes_on() -> Outcome {
    let before = crate::ticks();
    crate::sleep(2);
    let after = crate::ticks();
    expect(
        after >= before + 2,
        "two ticks should pass in a sleep of two",
    )?;
    crate::yield_now();
    expect(crate::ticks() >= after, "time should not go back")
}

fn round_trip() -> Outcome {
    let (a, b) = pair()?;
    expect(
        a != b && a.0 != 0 && b.0 != 0,
        "the ends should be two handles",
    )?;
    let mut room = [0u8; 16];
    expect(send(a, b"ping", None) == Ok(()), "a message should go out")?;
    expect(
        receive(b, &mut room) == Ok((4, None)) && room[..4] == *b"ping",
        "the message should arrive whole and alone",
    )?;
    expect(send(b, b"pong!", None) == Ok(()), "the other way too")?;
    expect(
        receive(a, &mut room) == Ok((5, None)) && room[..5] == *b"pong!",
        "the answer should arrive",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn nothing_waits() -> Outcome {
    let (a, b) = pair()?;
    let mut room = [0u8; 4];
    expect(
        receive(b, &mut room) == Err(Error::Empty),
        "an empty channel should say so",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn room_too_small() -> Outcome {
    let (a, b) = pair()?;
    expect(
        send(a, b"12345678", None) == Ok(()),
        "a message should go out",
    )?;
    let mut small = [0u8; 4];
    expect(
        receive(b, &mut small) == Err(Error::TooSmall),
        "four bytes should not hold eight",
    )?;
    let mut room = [0u8; 8];
    expect(
        receive(b, &mut room) == Ok((8, None)) && room == *b"12345678",
        "the message should wait until there is room",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn longest_message() -> Outcome {
    let (a, b) = pair()?;
    let longest = [b'm'; MESSAGE_BYTES + 1];
    expect(
        send(a, &longest, None) == Err(Error::TooLong),
        "a message longer than a channel takes should be refused",
    )?;
    expect(
        send(a, &longest[..MESSAGE_BYTES], None) == Ok(()),
        "the longest message should go",
    )?;
    let mut room = [0u8; MESSAGE_BYTES];
    expect(
        receive(b, &mut room) == Ok((MESSAGE_BYTES, None)) && room == longest[..MESSAGE_BYTES],
        "and arrive whole",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn handle_carried() -> Outcome {
    let (sender, receiver) = pair()?;
    let (kept, far) = pair()?;
    expect(
        send(sender, b"x", Some(kept)) == Ok(()),
        "a handle should go along",
    )?;
    let mut room = [0u8; 8];
    let Ok((1, Some(arrived))) = receive(receiver, &mut room) else {
        return Err("the handle should arrive with its message");
    };
    expect(
        close(kept) == Err(Error::BadHandle),
        "the handle sent should be gone from the sender",
    )?;
    expect(
        send(arrived, b"through", None) == Ok(()),
        "the handle received should work",
    )?;
    expect(
        receive(far, &mut room) == Ok((7, None)) && room[..7] == *b"through",
        "and reach the channel it stands for",
    )?;
    expect(
        [sender, receiver, far, arrived]
            .into_iter()
            .all(|h| close(h) == Ok(())),
        "every end should close",
    )
}

fn other_end_closed() -> Outcome {
    let (a, b) = pair()?;
    expect(close(a) == Ok(()), "one end should close")?;
    expect(
        send(b, b"anyone?", None) == Err(Error::Closed),
        "a message to a closed end should be refused",
    )?;
    let mut room = [0u8; 8];
    expect(
        receive(b, &mut room) == Err(Error::Closed),
        "nothing should come from a closed end",
    )?;
    expect(close(b) == Ok(()), "the other end should close")
}

fn full() -> Outcome {
    let (a, b) = pair()?;
    let mut sent = 0;
    while send(a, b"more", None) == Ok(()) {
        sent += 1;
        if sent > 1000 {
            return Err("a channel should fill up");
        }
    }
    expect(
        send(a, b"more", None) == Err(Error::Full),
        "a full channel should say so",
    )?;
    let mut room = [0u8; 4];
    expect(
        receive(b, &mut room) == Ok((4, None)),
        "a message should leave",
    )?;
    expect(
        send(a, b"more", None) == Ok(()),
        "and make room for one more",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn closed_twice() -> Outcome {
    let (a, b) = pair()?;
    expect(close(a) == Ok(()), "a handle should close")?;
    expect(
        close(a) == Err(Error::BadHandle),
        "a closed handle should be gone",
    )?;
    expect(close(b) == Ok(()), "the other end should close")
}

fn pages_come_and_go() -> Outcome {
    let first = map(2).map_err(|_| "two pages should be given")?;
    let second = map(1).map_err(|_| "and one more")?;
    expect(
        first % 4096 == 0 && first >= 0x10_0000_0000 && second >= first + 2 * 4096,
        "pages should come on whole pages, from the heap region, one piece after another",
    )?;
    expect(unmap(first + 4096, 1) == Ok(()), "a part should go back")?;
    expect(
        unmap(first, 2) == Err(Error::BadAddress),
        "pages that went back already should not go again",
    )?;
    expect(unmap(first, 1) == Ok(()), "the rest should go back")?;
    expect(unmap(second, 1) == Ok(()), "and the other piece")?;
    let three = map(3).map_err(|_| "three pages should be given")?;
    expect(unmap(three, 3) == Ok(()), "three should go back at once")?;
    expect(
        (0..3).all(|i| unmap(three + i * 4096, 1) == Err(Error::BadAddress)),
        "and every one of them be gone",
    )?;
    expect(
        map(1).is_ok_and(|again| again > second),
        "what went back should not be given again",
    )
}

fn sizes_there_are() -> Outcome {
    expect(map(0) == Err(Error::BadSize), "no pages are no request")?;
    expect(
        map(16385) == Err(Error::BadSize),
        "one call gives at most 64 MiB",
    )?;
    expect(
        unmap(0x40_0000, 1) == Err(Error::BadAddress),
        "the program's image is not the heap's to give back",
    )?;
    expect(
        unmap(0x10_0000_0000, 0) == Err(Error::BadAddress),
        "nothing cannot go back either",
    )
}

fn heap_in_use() -> Outcome {
    let numbers: Vec<u64> = (1..=1000).collect();
    expect(
        numbers.iter().sum::<u64>() == 500_500,
        "a vector should hold what was pushed",
    )?;
    let text = alloc::format!("{} numbers, the last {}", numbers.len(), numbers[999]);
    expect(
        text == "1000 numbers, the last 1000",
        "a string should be formatted",
    )?;
    drop(numbers);
    let page = Box::new([7u8; 4096]);
    expect(page.iter().all(|&b| b == 7), "a box should hold a page")?;
    let mut big: Vec<u8> = Vec::new();
    expect(
        big.try_reserve(2 << 20).is_err(),
        "more than the heap holds should be refused, not crash",
    )?;
    let mut grown = String::new();
    for _ in 0..1000 {
        grown.push_str("room ");
    }
    expect(grown.len() == 5000, "a string should grow")?;
    for round in 0..64u8 {
        let block = alloc::vec![round; 64 * 1024];
        expect(
            block[65_535] == round,
            "a block should hold what it was made of",
        )?;
    }
    Ok(())
}

/// Ends the start messages over `mine` to the child holding the other
/// end, sending none before the end, as a parent may (decision record
/// 0017).
fn begin(mine: Handle) -> Outcome {
    expect(
        send(mine, &[crate::start::Kind::End.byte()], None) == Ok(()),
        "the start messages should end",
    )
}

/// Starts exitwith with one end of a fresh channel, and returns the
/// process and the other end.
fn exitwith() -> Result<(Handle, Handle), &'static str> {
    let (mine, theirs) = pair()?;
    let child = spawn("exitwith.mexec", Some(theirs)).map_err(|_| "exitwith should start")?;
    begin(mine)?;
    expect(
        close(theirs) == Err(Error::BadHandle),
        "the end handed over should be gone from the parent",
    )?;
    Ok((child, mine))
}

fn child_exits_with_what_it_was_sent() -> Outcome {
    let (child, end) = exitwith()?;
    expect(
        send(end, &42u64.to_le_bytes(), None) == Ok(()),
        "the number should go to the child",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(42)),
        "the child should exit with the number it was sent",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(42)),
        "and say so again while the handle is held",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

fn child_runs_out_of_stack() -> Outcome {
    let (child, end) = exitwith()?;
    expect(send(end, b"!", None) == Ok(()), "the message should go")?;
    expect(
        wait(child) == Ok(Ended::Faulted(14)),
        "a child that runs out of stack should end by a page fault",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

fn child_ended_from_outside() -> Outcome {
    let (child, end) = exitwith()?;
    expect(
        terminate(child, 77) == Ok(()),
        "a child should be asked to end",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(77)),
        "and end with the code it was asked to end with",
    )?;
    expect(
        terminate(child, 78) == Ok(()) && wait(child) == Ok(Ended::Exited(77)),
        "a child that ended should stay as it ended",
    )?;
    expect(
        send(end, b"late", None) == Err(Error::Closed),
        "and its end of the channel should be closed",
    )?;
    let (spinning, its) = exitwith()?;
    expect(
        send(its, b"", None) == Ok(()),
        "the child should be told to spin",
    )?;
    sleep(2);
    expect(
        terminate(spinning, 79) == Ok(()) && wait(spinning) == Ok(Ended::Exited(79)),
        "a child that never calls the kernel should still end at the next interrupt",
    )?;
    expect(
        close(spinning) == Ok(()) && close(its) == Ok(()),
        "its handles should close",
    )?;
    let (later, its) = exitwith()?;
    let (waiting, theirs) = exitwith()?;
    expect(
        send(theirs, b"wait", Some(later)) == Ok(()),
        "a child should be handed another to wait for",
    )?;
    sleep(2);
    expect(
        terminate(waiting, 81) == Ok(()) && wait(waiting) == Ok(Ended::Exited(81)),
        "a child that waits for another should still end once asked to",
    )?;
    expect(
        send(its, &5u64.to_le_bytes(), None) == Ok(()),
        "the other one should still be there to end on its own",
    )?;
    expect(
        close(waiting) == Ok(()) && close(theirs) == Ok(()) && close(its) == Ok(()),
        "their handles should close",
    )?;
    expect(
        terminate(end, 1) == Err(Error::Denied),
        "a channel is no process to end",
    )?;
    expect(
        terminate(Handle(0x7777_0042), 1) == Err(Error::BadHandle),
        "nor is a handle never given",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

/// Starts the threads program, tells it `word`, and answers its process
/// and the end of the channel it sends on.
fn threads(word: &[u8]) -> Result<(Handle, Handle), &'static str> {
    let (mine, theirs) = pair()?;
    let child = spawn("threads.mexec", Some(theirs)).map_err(|_| "threads should start")?;
    begin(mine)?;
    expect(send(mine, word, None) == Ok(()), "the word should go")?;
    Ok((child, mine))
}

fn threads_of_a_child() -> Outcome {
    let (child, end) = threads(b"sum")?;
    let mut got = 0;
    let mut room = [0u8; 8];
    for _ in 0..1000 {
        match receive(end, &mut room) {
            Ok((8, _)) => got |= 1 << u64::from_le_bytes(room),
            Ok(_) => return Err("each thread should send its number"),
            Err(_) => sleep(1),
        }
        if got == 0b11110 {
            break;
        }
    }
    expect(got == 0b11110, "four threads should each send their number")?;
    expect(
        wait(child) == Ok(Ended::Exited(0)),
        "and the program exit with 0 once they all did",
    )?;
    let ended = |word: &[u8], how: Ended| {
        threads(word).is_ok_and(|(child, end)| {
            wait(child) == Ok(how) && close(child).is_ok() && close(end).is_ok()
        })
    };
    expect(
        ended(b"last", Ended::Exited(7)),
        "a program should end with its last thread, with that one's code",
    )?;
    expect(
        ended(b"exit", Ended::Exited(9)),
        "an exit should end every thread of a program",
    )?;
    expect(
        ended(b"fault", Ended::Faulted(14)),
        "a fault in one thread should end the whole program",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

/// What hello's own second thread found.
static FOUND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn found(n: u64) -> u64 {
    FOUND.store(n * 2, core::sync::atomic::Ordering::Release);
    0
}

fn a_thread_of_its_own() -> Outcome {
    expect(
        crate::thread(|| found(21)) == Ok(()),
        "hello should start a thread of its own",
    )?;
    let seen = (0..100).any(|_| {
        sleep(1);
        FOUND.load(core::sync::atomic::Ordering::Acquire) == 42
    });
    expect(seen, "which should run beside it, in its memory")
}

/// What the thread with the deep stack read back from both ends of it.
static DEEPEST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Fills all but 4 KiB of a thread's stack, a byte on every page, and
/// says what it reads back at both ends. A stack smaller than promised
/// runs into the page below it, and the fault ends hello.
fn deep() -> u64 {
    let mut room = [0u8; 60 << 10];
    let room = core::hint::black_box(&mut room);
    for page in room.chunks_mut(4096) {
        page[0] = 7;
    }
    let ends = u64::from(room[0]) + u64::from(room[room.len() - 4096]);
    DEEPEST.store(ends, core::sync::atomic::Ordering::Release);
    0
}

fn a_thread_has_all_its_stack() -> Outcome {
    expect(
        crate::thread(deep) == Ok(()),
        "a thread should start for a deep stack",
    )?;
    let seen = (0..100).any(|_| {
        sleep(1);
        DEEPEST.load(core::sync::atomic::Ordering::Acquire) == 14
    });
    expect(
        seen,
        "and have the 64 KiB its stack is promised, all but 4 KiB of it in use",
    )
}

/// What the thread that was given a list found in it.
static SUMMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn a_thread_takes_what_it_was_given() -> Outcome {
    let numbers: Vec<u64> = (1..=100).collect();
    let (mine, theirs) = pair()?;
    let started = crate::thread(move || {
        let sum: u64 = numbers.iter().sum();
        SUMMED.store(sum, core::sync::atomic::Ordering::Release);
        let sent = send(theirs, &numbers.len().to_le_bytes(), None);
        u64::from(sent.is_err())
    });
    expect(
        started == Ok(()),
        "a thread should start with a list of its own",
    )?;
    let mut room = [0u8; 8];
    let got = (0..100).find_map(|_| {
        if let Ok((8, _)) = receive(mine, &mut room) {
            return Some(u64::from_le_bytes(room));
        }
        sleep(1);
        None
    });
    expect(
        got == Some(100) && SUMMED.load(core::sync::atomic::Ordering::Acquire) == 5050,
        "it should find every number it was given, in the memory it was moved to",
    )?;
    expect(
        close(mine) == Ok(()) && close(theirs) == Ok(()),
        "the channel's ends should close",
    )
}

fn no_such_program() -> Outcome {
    expect(
        spawn("nothing.mexec", None) == Err(Error::NoSuchProgram),
        "a name no file has should start nothing",
    )?;
    expect(
        spawn("", None) == Err(Error::NoSuchProgram),
        "neither should no name",
    )?;
    expect(
        spawn("a-name-much-longer-than-a-module-keeps.mexec", None) == Err(Error::NoSuchProgram),
        "nor one longer than any file's",
    )?;
    expect(
        spawn("exitwith.mexec", Some(Handle(0x7777_0042))) == Err(Error::BadHandle),
        "a handle the parent does not hold cannot be handed over",
    )
}

fn own_token() -> Outcome {
    expect(
        token() == Some(Token::system()),
        "hello, started by the kernel, should run as SYSTEM with every privilege",
    )?;
    let mut room = [0u8; Token::MAX_SIZE];
    let size = Token::system().size();
    let short = abi::Call::Token {
        room: size as u64 - 1,
        at: room.as_mut_ptr() as u64,
    };
    expect(
        abi::answer(raw::call(short).0) == Err(Error::TooSmall),
        "a byte too little room should take nothing",
    )?;
    let exact = abi::Call::Token {
        room: size as u64,
        at: room.as_mut_ptr() as u64,
    };
    expect(
        abi::answer(raw::call(exact).0) == Ok(size as u64)
            && Token::read(&room[..size]) == Some(Token::system()),
        "room for the token and no more should do",
    )?;
    let kernels = abi::Call::Token {
        room: size as u64,
        at: abi::USER_END,
    };
    expect(
        abi::answer(raw::call(kernels).0) == Err(Error::BadAddress),
        "the kernel's half is no room of the program's",
    )
}

/// What whoami, started with `restriction`, sends as its token.
fn whoami(restriction: Option<Restriction>) -> Result<Token, &'static str> {
    let (mine, theirs) = pair()?;
    let child = match restriction {
        None => spawn("whoami.mexec", Some(theirs)),
        Some(r) => spawn_restricted("whoami.mexec", Some(theirs), r),
    }
    .map_err(|_| "whoami should start")?;
    begin(mine)?;
    let ended = wait(child);
    let mut room = [0u8; MESSAGE_BYTES];
    let got = receive(mine, &mut room);
    let closed = close(child) == Ok(()) && close(mine) == Ok(());
    match (ended, got, closed) {
        (Ok(Ended::Exited(0)), Ok((len, None)), true) => {
            Token::read(&room[..len]).ok_or("whoami's token should read back")
        }
        _ => Err("whoami should send its token and exit"),
    }
}

fn child_gets_no_more() -> Outcome {
    expect(
        whoami(None) == Ok(Token::system()),
        "a child should get its parent's token as it is",
    )?;
    let low = Restriction {
        integrity: Integrity::Low,
        keep: Privileges::SHUTDOWN,
    };
    let less = whoami(Some(low))?;
    expect(
        less.integrity == Integrity::Low
            && less.privileges == Privileges::SHUTDOWN
            && less == Token::system().restricted(low),
        "a restricted child should run low, with only the privilege it kept",
    )?;
    let bad = abi::Call::Spawn {
        len: 12,
        at: "whoami.mexec".as_ptr() as u64,
        carry: 0,
        restriction: 1,
    };
    expect(
        abi::answer(raw::call(bad).0) == Err(Error::BadArgument),
        "a word that holds no restriction should start nothing",
    )
}

/// The settings sextant writes into test images.
const SETTINGS: &[u8] =
    b"boot {\n    timeout = 2500ms\n    default = example\n    verbose = true\n}\n";

/// Whether the boot volume hangs in the tree, as it does whenever the
/// machine started from a disk; the kernel's own check wants it then.
fn booted_from_disk() -> bool {
    open("/System/Boot", 0).is_ok_and(|boot| close(boot).is_ok())
}

fn child_from_its_file() -> Outcome {
    let exitwith = "/System/Boot/meridian/programs/exitwith.mexec";
    if !booted_from_disk() {
        return expect(
            spawn(exitwith, None) == Err(Error::NotFound),
            "booted from a CD, there should be no file to start",
        );
    }
    let (mine, theirs) = pair()?;
    let child = spawn(exitwith, Some(theirs)).map_err(|_| "exitwith should start from its file")?;
    begin(mine)?;
    expect(
        send(mine, &7u64.to_le_bytes(), None) == Ok(()),
        "the number should go to the child",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(7)),
        "the child started from its file should exit with the number it was sent",
    )?;
    expect(
        close(child) == Ok(()) && close(mine) == Ok(()),
        "the handles should close",
    )?;
    expect(
        spawn("/System/Boot/meridian/programs", None) == Err(Error::NotAFile),
        "a directory is no program",
    )?;
    expect(
        spawn("/System/Boot/meridian/boot.meri", None) == Err(Error::NotAProgram),
        "nor are the loader's settings",
    )?;
    expect(
        spawn("/System/Boot/meridian/programs/nothing.mexec", None) == Err(Error::NotFound),
        "a path to no file starts nothing",
    )?;
    let longest = alloc::format!("/{}", "a".repeat(abi::PATH_BYTES - 1));
    expect(
        spawn(&longest, None) == Err(Error::NotFound)
            && spawn(&alloc::format!("{longest}b"), None) == Err(Error::TooLong),
        "a path as long as a path may be is looked for, and one longer refused",
    )
}

fn settings_read_back() -> Outcome {
    if !booted_from_disk() {
        return expect(
            open("/System/Boot/meridian/boot.meri", abi::file::READ) == Err(Error::NotFound),
            "booted from a CD, there should be no boot.meri to open",
        );
    }
    let file = open("/System/Boot/meridian/boot.meri", abi::file::READ)
        .map_err(|_| "boot.meri should open from the EFI system partition")?;
    expect(
        info(file)
            == Ok(Info {
                size: SETTINGS.len() as u64,
                directory: false,
            }),
        "its size should be the one sextant wrote",
    )?;
    let mut room = [0u8; 128];
    expect(
        read(file, 0, &mut room) == Ok(SETTINGS.len()) && room[..SETTINGS.len()] == *SETTINGS,
        "it should read as sextant wrote it",
    )?;
    expect(
        read(file, 11, &mut room[..7]) == Ok(7) && room[..7] == *b"timeout",
        "from a byte in the middle on",
    )?;
    expect(
        read(file, SETTINGS.len() as u64, &mut room) == Ok(0),
        "and nothing from its end on",
    )?;
    expect(close(file) == Ok(()), "the file should close")?;
    let again = open("/system/BOOT/Meridian/BOOT.MERI", abi::file::READ)
        .map_err(|_| "a path in other capitals should name the same file")?;
    expect(
        read(again, 0, &mut room) == Ok(SETTINGS.len()) && close(again) == Ok(()),
        "and read the same",
    )
}

fn files_that_do_not_open() -> Outcome {
    expect(
        open("/System/Boot/meridian/nothing", abi::file::READ) == Err(Error::NotFound),
        "a name no file has should open nothing",
    )?;
    expect(
        open("/System/Boot/meridian/boot.meri/inside", abi::file::READ) == Err(Error::NotFound),
        "nor a name inside a file",
    )?;
    expect(
        open("/Nowhere/x", abi::file::READ) == Err(Error::NotFound),
        "nor a path no volume hangs at",
    )?;
    let opened = |path: &str| {
        abi::answer(
            raw::call(abi::Call::Open {
                len: path.len() as u64,
                at: path.as_ptr() as u64,
                wanted: u64::from(abi::file::READ),
                how: 0,
            })
            .0,
        )
    };
    expect(
        opened("meridian/boot.meri") == Err(Error::BadArgument)
            && opened("/System/../..") == Err(Error::BadArgument),
        "the kernel should take a path not from the root, or above it, as none",
    )?;
    expect(
        open("meridian/boot.meri", abi::file::READ) == Err(Error::NotFound)
            && crate::full_path("meridian/boot.meri") == "/meridian/boot.meri",
        "the runtime should start a path not from the root at the working directory, the root here",
    )?;
    let long = [b'/'; abi::PATH_BYTES + 1];
    let too_long = abi::Call::Open {
        len: long.len() as u64,
        at: long.as_ptr() as u64,
        wanted: u64::from(abi::file::READ),
        how: 0,
    };
    expect(
        abi::answer(raw::call(too_long).0) == Err(Error::TooLong),
        "a path longer than PATH_BYTES",
    )?;
    let mut longest = [b'a'; abi::PATH_BYTES];
    longest[0] = b'/';
    let just = abi::Call::Open {
        len: longest.len() as u64,
        at: longest.as_ptr() as u64,
        wanted: u64::from(abi::file::READ),
        how: 0,
    };
    expect(
        abi::answer(raw::call(just).0) == Err(Error::NotFound),
        "a path of PATH_BYTES is still one, if of no file",
    )?;
    let stranger = Handle(0x7777_0042);
    let mut room = [0u8; 8];
    expect(
        read(stranger, 0, &mut room) == Err(Error::BadHandle)
            && info(stranger) == Err(Error::BadHandle)
            && list(stranger, 0, &mut room) == Err(Error::BadHandle),
        "a made up handle is no file to read, ask about or list",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        open("/System/Boot/meridian/boot.meri", abi::file::WRITE) == Err(Error::ReadOnly),
        "nothing writes to a volume mounted read only, and it says so",
    )?;
    let directory =
        open("/System/Boot/meridian", abi::file::READ).map_err(|_| "a directory should open")?;
    expect(
        info(directory).is_ok_and(|i| i.directory)
            && read(directory, 0, &mut room) == Err(Error::NotAFile)
            && close(directory) == Ok(()),
        "a directory should say it is one, and have no bytes to read",
    )?;
    let blind =
        open("/System/Boot/meridian/boot.meri", 0).map_err(|_| "a file should open for nothing")?;
    expect(
        read(blind, 0, &mut room) == Err(Error::Denied) && close(blind) == Ok(()),
        "a handle opened without the right to read should not read",
    )
}

fn directories_list_their_entries() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let meridian = open("/System/Boot/meridian", abi::file::READ)
        .map_err(|_| "the meridian directory should open")?;
    let mut names = Vec::new();
    let mut room = [0u8; 64];
    for index in 0.. {
        match list(meridian, index, &mut room) {
            Ok((len, directory)) => names.push((
                String::from_utf8_lossy(&room[..len]).into_owned(),
                directory,
            )),
            Err(Error::NotFound) => break,
            Err(_) => return Err("each entry should list until the last"),
        }
    }
    let has = |name: &str, directory: bool| names.iter().any(|(n, d)| n == name && *d == directory);
    expect(
        has("boot.meri", false) && has("kernel", false) && has("programs", true),
        "the meridian directory should list boot.meri, the kernel and the programs' directory",
    )?;
    expect(
        list(meridian, 0, &mut room[..1]) == Err(Error::TooSmall),
        "a name should not be cut to fit",
    )?;
    let settings = open("/System/Boot/meridian/boot.meri", abi::file::READ)
        .map_err(|_| "boot.meri should open")?;
    expect(
        list(settings, 0, &mut room) == Err(Error::NotADirectory),
        "a file has no entries",
    )?;
    let blind =
        open("/System/Boot/meridian", 0).map_err(|_| "a directory should open for nothing")?;
    expect(
        list(blind, 0, &mut room) == Err(Error::Denied),
        "a handle opened without the right to read should not list",
    )?;
    expect(
        close(meridian) == Ok(()) && close(settings) == Ok(()) && close(blind) == Ok(()),
        "the handles should close",
    )
}

/// A file read through `text::Source` stays open, so removing it is
/// refused, until the source closes it, and then it goes.
fn sources_close_their_files() -> Outcome {
    use abi::file::{CREATE_NEW, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/source.txt";
    match open_with(path, WRITE, CREATE_NEW) {
        Err(Error::ReadOnly) => return Ok(()),
        Err(_) => return Err("a file should be made on /System"),
        Ok(file) => {
            expect(close(file) == Ok(()), "the new file should close")?;
        }
    }
    let source = crate::text::Source::open(path).map_err(|_| "the file should open as a source")?;
    expect(
        remove(path) == Err(Error::InUse),
        "a file read through a source should stay while it is open",
    )?;
    source.close();
    expect(
        remove(path) == Ok(()),
        "once its source closed it, the file should go",
    )
}

/// On /System: a file made, written in more pieces than one call takes,
/// read back, and removed once closed; a directory made and removed.
/// Where /System hangs read only, as on a disk that only reads, nothing is
/// made or removed there.
fn files_written() -> Outcome {
    use abi::file::{CREATE_NEW, READ, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/ring3.txt";
    let file = match open_with(path, READ | WRITE, CREATE_NEW) {
        Err(Error::ReadOnly) => {
            return expect(
                make_directory("/System/Ring3") == Err(Error::ReadOnly)
                    && remove("/System/made.meri") == Err(Error::ReadOnly),
                "on /System read only nothing should be made or removed",
            );
        }
        other => other.map_err(|_| "a file should be made on /System")?,
    };
    let other = open("/System/made.meri", READ).map_err(|_| "made.meri should open")?;
    let size = info(other).map(|i| i.size);
    let bytes: Vec<u8> = (0..abi::WRITE_BYTES + 100)
        .map(|i| b'a' + (i % 26).to_le_bytes()[0])
        .collect();
    expect(
        write_file(file, 0, &bytes) == Ok(()),
        "a file should take more than one call writes",
    )?;
    let mut back = alloc::vec![0u8; bytes.len()];
    let mut done = 0;
    while let Ok(n @ 1..) = read(file, done as u64, &mut back[done..]) {
        done += n;
    }
    expect(
        back == bytes && info(file).map(|i| i.size) == Ok(bytes.len() as u64),
        "the file should read back as written",
    )?;
    expect(
        info(other).map(|i| i.size) == size && close(other) == Ok(()),
        "another file open on the volume should keep its size",
    )?;
    let too_long = abi::Call::WriteFile {
        handle: file.0,
        at: bytes.as_ptr() as u64,
        len: (abi::WRITE_BYTES + 1) as u64,
        offset: 0,
    };
    expect(
        abi::answer(raw::call(too_long).0) == Err(Error::TooLong),
        "one call should take at most WRITE_BYTES",
    )?;
    let reader = open(path, READ).map_err(|_| "the file should open again")?;
    let too_long = abi::Call::WriteFile {
        handle: reader.0,
        at: bytes.as_ptr() as u64,
        len: (abi::WRITE_BYTES + 1) as u64,
        offset: 0,
    };
    expect(
        write_file(reader, 0, b"x") == Err(Error::Denied)
            && abi::answer(raw::call(too_long).0) == Err(Error::Denied)
            && remove(path) == Err(Error::InUse),
        "a handle that may only read should not write, whatever it hands over, and an open file should stay",
    )?;
    expect(
        close(reader) == Ok(()) && close(file) == Ok(()),
        "the handles should close",
    )?;
    expect(
        remove(path) == Ok(()) && open(path, READ) == Err(Error::NotFound),
        "a closed file should go",
    )?;
    expect(
        make_directory("/System/Ring3") == Ok(())
            && make_directory("/System/Ring3") == Err(Error::Exists)
            && remove("/System/Ring3") == Ok(()),
        "a directory should be made once and removed",
    )
}

/// What takes no writes wherever the program runs: /System/Boot, a way of
/// opening that is none, a handle that is not the program's, and a path
/// longer than a call takes.
fn writes_refused() -> Outcome {
    use abi::file::{CREATE_NEW, READ, WRITE};
    if booted_from_disk() {
        expect(
            open_with("/System/Boot/new.txt", READ | WRITE, CREATE_NEW) == Err(Error::ReadOnly)
                && remove("/System/Boot/meridian/boot.meri") == Err(Error::ReadOnly)
                && make_directory("/System/Boot/New") == Err(Error::ReadOnly),
            "/System/Boot should take no writes",
        )?;
        expect(
            open_with("/System/Boot/meridian/boot.meri", READ, 6) == Err(Error::BadArgument),
            "there is no sixth way to open a file",
        )?;
    }
    let long = alloc::format!("/{}", "a".repeat(abi::PATH_BYTES));
    expect(
        write_file(Handle(0x7777_0042), 0, b"x") == Err(Error::BadHandle)
            && remove(&long) == Err(Error::TooLong)
            && make_directory(&long) == Err(Error::TooLong),
        "a stranger's handle should not write, nor a path past PATH_BYTES name anything",
    )
}

/// A duplicate stands for what its handle stands for, which closes with
/// the last of them: a channel end, an open file and a process.
fn duplicates_stand_for_the_same_object() -> Outcome {
    let (mine, theirs) = pair()?;
    let copy = duplicate(mine).map_err(|_| "a channel end should be duplicated")?;
    let mut room = [0u8; 8];
    expect(
        copy != mine
            && send(copy, b"a", None) == Ok(())
            && send(mine, b"b", None) == Ok(())
            && receive(theirs, &mut room) == Ok((1, None))
            && room[0] == b'a'
            && receive(theirs, &mut room) == Ok((1, None))
            && room[0] == b'b',
        "both handles should send through the one end",
    )?;
    expect(
        close(mine) == Ok(())
            && send(theirs, b"c", None) == Ok(())
            && receive(copy, &mut room) == Ok((1, None))
            && room[0] == b'c',
        "the end should stay open while its duplicate does",
    )?;
    expect(
        close(copy) == Ok(())
            && send(theirs, b"d", None) == Err(Error::Closed)
            && receive(theirs, &mut room) == Err(Error::Closed)
            && close(theirs) == Ok(()),
        "the end should close with its last handle",
    )?;
    let (child, line) = exitwith()?;
    let watcher = duplicate(child).map_err(|_| "a process should be duplicated")?;
    expect(
        send(line, &5u64.to_le_bytes(), None) == Ok(())
            && wait(child) == Ok(Ended::Exited(5))
            && close(child) == Ok(())
            && wait(watcher) == Ok(Ended::Exited(5))
            && close(watcher) == Ok(())
            && close(line) == Ok(()),
        "a duplicate should wait for the process after the first handle closed",
    )?;
    expect(
        duplicate(Handle(0x7777_0042)) == Err(Error::BadHandle),
        "a stranger's handle should not be duplicated",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let file = open("/System/Boot/meridian/boot.meri", abi::file::READ)
        .map_err(|_| "boot.meri should open")?;
    let again = duplicate(file).map_err(|_| "a file should be duplicated")?;
    let size = info(file).map(|i| i.size);
    expect(
        close(file) == Ok(())
            && info(again).map(|i| i.size) == size
            && read(again, 0, &mut room).is_ok_and(|n| n > 0)
            && close(again) == Ok(())
            && info(again) == Err(Error::BadHandle),
        "an open file should read through its duplicate after the first handle closed",
    )
}

/// A file or a directory moves within its volume, and keeps what it holds;
/// what cannot move says why.
fn files_renamed() -> Outcome {
    use abi::file::{CREATE_NEW, READ, WRITE};
    let long = alloc::format!("/{}", "a".repeat(abi::PATH_BYTES));
    expect(
        rename(&long, "/System/x") == Err(Error::TooLong)
            && rename("/System/x", &long) == Err(Error::TooLong)
            && rename("/System/a\0b", "/System/y") == Err(Error::BadArgument),
        "paths past PATH_BYTES or that are none should move nothing",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        rename("/System/Boot/meridian/boot.meri", "/System/boot.meri") == Err(Error::NotSameVolume)
            && rename("/System/Boot/meridian/boot.meri", "/System/Boot/b.meri")
                == Err(Error::ReadOnly),
        "a file should move neither to another volume nor on one only read",
    )?;
    let file = match open_with("/System/r.txt", READ | WRITE, CREATE_NEW) {
        Err(Error::ReadOnly) => {
            return expect(
                rename("/System/made.meri", "/System/m.meri") == Err(Error::ReadOnly),
                "on /System read only nothing should move",
            );
        }
        other => other.map_err(|_| "a file should be made to move")?,
    };
    let written = write_file(file, 0, b"moved");
    expect(
        written == Ok(()) && rename("/System/r.txt", "/System/r2.txt") == Err(Error::InUse),
        "an open file should stay where it is",
    )?;
    let _ = close(file);
    expect(
        make_directory("/System/RDir") == Ok(())
            && rename("/System/r.txt", "/System/RDir/r2.txt") == Ok(())
            && open("/System/r.txt", READ) == Err(Error::NotFound),
        "a closed file should move into a directory",
    )?;
    let moved = open("/System/rdir/R2.TXT", READ).map_err(|_| "the moved file should open")?;
    let mut room = [0u8; 8];
    let len = read(moved, 0, &mut room).unwrap_or(0);
    let _ = close(moved);
    expect(
        room[..len] == *b"moved",
        "the moved file should hold what it held",
    )?;
    expect(
        rename("/System/RDir", "/System/RDir/Inner") == Err(Error::Inside)
            && rename("/System/nope", "/System/x") == Err(Error::NotFound)
            && rename("/System/RDir", "/System/made.meri") == Err(Error::Exists),
        "a directory should not go inside itself, nor anything onto what is there",
    )?;
    expect(
        rename("/System/RDir", "/System/rdir") == Ok(())
            && remove("/System/rdir/r2.txt") == Ok(())
            && remove("/System/rdir") == Ok(()),
        "a directory should take other capitals, and go once empty",
    )
}

/// The working directory changes to a directory that is there and to
/// nothing else, paths start there, and a child starts in its parent's.
fn working_directory() -> Outcome {
    // Whatever went wrong, the checks after start at the root again.
    let checked = directory_moves();
    let back = crate::change_directory("/");
    checked?;
    expect(
        back == Ok(()) && crate::directory() == "/",
        "the program should go back to the root",
    )
}

fn directory_moves() -> Outcome {
    use abi::file::READ;
    expect(
        crate::directory() == "/"
            && crate::change_directory("/nope") == Err(Error::NotFound)
            && crate::change_directory("/") == Ok(())
            && crate::directory() == "/",
        "a program should start at the root, and stay there when told to go nowhere",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        crate::change_directory("/System/Boot") == Ok(())
            && crate::directory() == "/System/Boot"
            && open("meridian/boot.meri", READ).is_ok_and(|h| close(h) == Ok(()))
            && crate::change_directory("meridian/boot.meri") == Err(Error::NotADirectory)
            && crate::directory() == "/System/Boot",
        "paths not from the root should start at the working directory, which takes no file",
    )?;
    let inherited = shell_on("pwd")?;
    expect(
        crate::change_directory("..") == Ok(())
            && crate::directory() == "/System"
            && crate::change_directory("/") == Ok(()),
        "the directory above should be a step away, and the root one more",
    )?;
    expect(
        inherited == ran(0, b"/System/Boot\n"),
        "a child should start in its parent's working directory",
    )
}

/// A child gets the words it is started with as they were, empty ones
/// and ones longer than a message too, and sends them back.
fn arguments_arrive() -> Outcome {
    let long = "\u{e9}".repeat(150);
    let sent = ["one", "", long.as_str(), "two words"];
    let (child, mine) =
        spawn_with("arguments.mexec", &sent).map_err(|_| "arguments should start")?;
    let mut got: Vec<Vec<u8>> = Vec::from([Vec::new()]);
    let mut room = [0u8; MESSAGE_BYTES];
    while got.len() <= sent.len() {
        match receive(mine, &mut room) {
            Ok((0, _)) => got.push(Vec::new()),
            Ok((len, _)) => got
                .last_mut()
                .into_iter()
                .for_each(|a| a.extend_from_slice(&room[..len])),
            Err(Error::Empty) => sleep(1),
            Err(_) => break,
        }
    }
    got.pop();
    let ended = wait(child);
    let closed = close(child) == Ok(()) && close(mine) == Ok(());
    expect(
        ended == Ok(Ended::Exited(0)) && closed && got.iter().eq(sent.iter().map(|a| a.as_bytes())),
        "the child should send back each word it was started with, as it was",
    )
}

/// What a child is given with `sink` as its standard output.
fn output(sink: start::Sink) -> start::Given {
    start::Given {
        output: Some(sink),
        ..start::Given::default()
    }
}

/// Takes every message waiting at `end` once the child that sent them
/// ended, and answers their bytes.
fn drained(end: Handle) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut room = [0u8; MESSAGE_BYTES];
    while let Ok((len, _)) = receive(end, &mut room) {
        bytes.extend_from_slice(&room[..len]);
    }
    bytes
}

/// What a child writes goes to the standard output it was handed: a
/// channel, and a file where /System takes one, at its end.
fn output_goes_where_it_is_handed() -> Outcome {
    let words = ["first", "second line"];
    let want = b"first\nsecond line\n";
    let (mine, theirs) = pair()?;
    let (child, back) = spawn_handing(
        "arguments.mexec",
        &words,
        output(start::Sink::Channel(theirs)),
    )
    .map_err(|_| "arguments should start with an output")?;
    let ended = wait(child);
    let _ = drained(back);
    expect(
        ended == Ok(Ended::Exited(0)) && drained(mine) == want,
        "what the child writes should come through the channel it was handed",
    )?;
    expect(
        close(child) == Ok(()) && close(back) == Ok(()) && close(mine) == Ok(()),
        "the handles should close",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/output.txt";
    let file = match open_with(path, abi::file::WRITE, abi::file::CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made for the output")?,
    };
    write_file(file, 0, b"before\n").map_err(|_| "the file should take a line")?;
    let (child, back) = spawn_handing("arguments.mexec", &words, output(start::Sink::File(file)))
        .map_err(|_| "arguments should start with a file")?;
    let ended = wait(child);
    let _ = drained(back);
    let _ = close(child);
    let _ = close(back);
    let reader = open(path, abi::file::READ).map_err(|_| "the output should open")?;
    let mut room = [0u8; 64];
    let len = read(reader, 0, &mut room).unwrap_or(0);
    let _ = close(reader);
    expect(
        ended == Ok(Ended::Exited(0)) && room[..len] == *b"before\nfirst\nsecond line\n",
        "what the child writes should go after what the file held",
    )?;
    expect(remove(path) == Ok(()), "the output should go")
}

/// How the shell ended and what it wrote.
type Ran = (Result<Ended, Error>, Vec<u8>);

/// Starts the shell with `words` and a channel as its standard output,
/// and answers how it ended and what it wrote, which has to fit in the
/// messages a channel holds, since nothing takes them until it ended.
fn shell_with(words: &[&str]) -> Result<Ran, &'static str> {
    shell_reading(words, None)
}

/// Starts the shell with `words` as [`shell_with`] does, and with a
/// standard input that brings `input` and ends, when that is some. What
/// the shell says went wrong comes through the same channel, in its order.
/// What it says is read once it ended, so a script says at most 8
/// messages, as many as a channel holds, or the shell waits for room for
/// ever: each echo, each line a program writes and each complaint is one.
fn shell_reading(words: &[&str], input: Option<&[u8]>) -> Result<Ran, &'static str> {
    let (mine, theirs) = pair()?;
    let mut given = output(start::Sink::Channel(theirs));
    given.error = Some(start::Sink::Channel(
        duplicate(theirs).map_err(|_| "the shell's output should be duplicated")?,
    ));
    let into = match input {
        Some(_) => {
            let (into, from) = pair()?;
            given.input = Some(from);
            Some(into)
        }
        None => None,
    };
    let (child, back) =
        spawn_handing("shell.mexec", words, given).map_err(|_| "the shell should start")?;
    if let (Some(into), Some(input)) = (into, input) {
        for piece in start::pieces(input) {
            let _ = crate::send_waiting(into, piece);
        }
        let _ = close(into);
    }
    let ended = wait(child);
    let said = drained(mine);
    expect(
        close(child) == Ok(()) && close(back) == Ok(()) && close(mine) == Ok(()),
        "the shell's handles should close",
    )?;
    Ok((ended, said))
}

/// Starts the shell with `words` as [`shell_with`] does, with a standard
/// input that brings `input` and then the empty message that ends a
/// stream, while the end that wrote it stays open until the shell ended.
fn shell_reading_to_the_end(words: &[&str], input: &[u8]) -> Result<Ran, &'static str> {
    let (mine, theirs) = pair()?;
    let (into, from) = pair()?;
    let mut given = output(start::Sink::Channel(theirs));
    given.input = Some(from);
    let (child, back) =
        spawn_handing("shell.mexec", words, given).map_err(|_| "the shell should start")?;
    for piece in start::pieces(input).chain([&[][..]]) {
        let _ = crate::send_waiting(into, piece);
    }
    let ended = wait(child);
    let said = drained(mine);
    expect(
        close(child) == Ok(())
            && close(back) == Ok(())
            && close(mine) == Ok(())
            && close(into) == Ok(()),
        "the shell's handles should close",
    )?;
    Ok((ended, said))
}

/// The shell run on the script `text`.
fn shell_on(text: &str) -> Result<Ran, &'static str> {
    shell_with(&["-c", text])
}

/// What a shell that ended with `status` after writing `said` answers.
fn ran(status: u64, said: &[u8]) -> Ran {
    (Ok(Ended::Exited(status)), said.to_vec())
}

/// Whether the shell ran as `want` says, logging how it ran when not, so a
/// check that fails shows what came instead.
fn same(got: &Ran, want: &Ran) -> bool {
    if got != want {
        let _ = crate::log!(
            "the shell ended {:?} after {:?}",
            got.0,
            String::from_utf8_lossy(&got.1)
        );
    }
    got == want
}

/// A child reads the standard input it is handed until an empty message
/// comes, or until nothing can write to it any more: wc, which counts it.
fn input_is_read_to_its_end() -> Outcome {
    for empty_at_the_end in [true, false] {
        let (into, from) = pair()?;
        let (mine, theirs) = pair()?;
        let given = start::Given {
            input: Some(from),
            output: Some(start::Sink::Channel(theirs)),
            ..start::Given::default()
        };
        let (child, back) = spawn_handing("wc.mexec", &[], given).map_err(|_| "wc should start")?;
        let sent = send(into, b"one two\n", None) == Ok(()) && send(into, b"three", None) == Ok(());
        let ended = if empty_at_the_end {
            let sent = send(into, b"", None) == Ok(());
            let ended = wait(child);
            sent.then_some(ended)
        } else {
            let closed = close(into) == Ok(());
            closed.then(|| wait(child))
        };
        expect(
            sent && ended == Some(Ok(Ended::Exited(0))) && drained(mine) == b"1 3 13\n",
            "wc should count its standard input to the empty message or to the closed end",
        )?;
        if empty_at_the_end {
            let _ = close(into);
        }
        let _ = (close(child), close(back), close(mine));
    }
    Ok(())
}

/// What a child says went wrong goes to the standard error it was handed,
/// apart from what it writes.
fn errors_go_apart() -> Outcome {
    let (out, theirs) = pair()?;
    let (errors, error_end) = pair()?;
    let given = start::Given {
        output: Some(start::Sink::Channel(theirs)),
        error: Some(start::Sink::Channel(error_end)),
        ..start::Given::default()
    };
    let (child, back) = spawn_handing("wc.mexec", &["/nope", "/nope/either"], given)
        .map_err(|_| "wc should start")?;
    let ended = wait(child);
    let (said, complained) = (drained(out), drained(errors));
    let _ = (close(child), close(back), close(out), close(errors));
    expect(
        ended == Ok(Ended::Exited(1))
            && said == b"0 0 0 total\n"
            && complained == b"wc: /nope: no such file\nwc: /nope/either: no such file\n",
        "a child should write its total to its output and what went wrong to its error",
    )
}

/// A stream's pieces come as sent, an empty one ends it as the other end
/// closing does, a handle one brings is closed, and a piece longer than
/// the room stays.
fn streams_end_and_close_what_they_bring() -> Outcome {
    let (mine, theirs) = pair()?;
    let (kept, brought) = pair()?;
    let mut room = [0u8; MESSAGE_BYTES];
    expect(
        send(theirs, b"one", None) == Ok(())
            && send(theirs, b"two", Some(brought)) == Ok(())
            && send(theirs, b"", None) == Ok(())
            && stream(mine, &mut room) == Ok(3)
            && room[..3] == *b"one"
            && stream(mine, &mut room) == Ok(3)
            && room[..3] == *b"two"
            && stream(mine, &mut room) == Ok(0),
        "pieces should come as sent, and an empty one end the stream",
    )?;
    expect(
        send(kept, b"x", None) == Err(Error::Closed) && close(kept) == Ok(()),
        "a handle a piece brought should be closed",
    )?;
    let mut small = [0u8; 2];
    expect(
        send(theirs, b"long", None) == Ok(())
            && stream(mine, &mut small) == Err(Error::TooSmall)
            && stream(mine, &mut room) == Ok(4),
        "a piece longer than the room should stay for a read with room enough",
    )?;
    expect(
        close(theirs) == Ok(()) && stream(mine, &mut room) == Ok(0) && close(mine) == Ok(()),
        "a stream should end once its other end closed",
    )?;
    expect(
        stream(Handle(0x7777_0042), &mut room) == Err(Error::BadHandle),
        "a stranger's handle should bring no stream",
    )
}

fn shell_runs_a_script() -> Outcome {
    let script = "let a = hi; echo $a there\nif false { echo no } else { echo yes }\n\
                  for x in 1 2 { echo $x }\nfn f { return 3 }\nf; echo $?";
    expect(
        shell_on(script)? == ran(0, b"hi there\nyes\n1\n2\n3\n"),
        "the shell should run the script after -c, its variables, blocks and functions",
    )
}

/// The shell's own commands, each as `help` names it, and what they say
/// when they are missing what they need.
fn the_shells_own_commands() -> Outcome {
    expect(
        shell_on("echo; echo  a   b")? == ran(0, b"\na b\n"),
        "echo should write its words with one space between",
    )?;
    expect(
        shell_on("md; rm")? == ran(1, b"md needs a path\nrm needs a path\n"),
        "md and rm without a path should say they need one",
    )?;
    expect(
        shell_on("services")? == ran(1, b"services: init did not start this shell\n"),
        "services should say init did not start a shell hello started",
    )?;
    expect(
        shell_on("clear | wc")? == ran(0, b"0 0 1\n"),
        "clear should write one form feed",
    )?;
    let listed = shell_on("cd /System/Boot; ls")?;
    let want: &[u8] = if booted_from_disk() {
        b"meridian/\n"
    } else {
        b"/System/Boot: no such file\n"
    };
    expect(
        listed.1.windows(want.len()).any(|w| w == want),
        "ls without a path should list the working directory",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        shell_on("ls /System/Boot/meridian/programs/wh*; dir /System/Boot/meridian/programs/wc*")?
            == ran(
                0,
                b"/System/Boot/meridian/programs/whoami.mexec\n\
                  /System/Boot/meridian/programs/whoami_again.mexec\n\
                  /System/Boot/meridian/programs/wc.mexec\n\
                  /System/Boot/meridian/programs/wc_again.mexec\n",
            ),
        "ls and dir of files should show their paths",
    )?;
    expect(
        shell_on(
            "type /System/Boot/meridian/boot.meri | wc; cat /System/Boot/meridian/boot.meri | wc",
        )?
        .1
        .split(|&b| b == b'\n')
        .take(2)
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[0] == w[1] && !w[0].is_empty()),
        "type should show a file as cat does",
    )
}

fn shell_ends_with_the_status() -> Outcome {
    expect(
        shell_on("echo a; exit 7; echo b")? == ran(7, b"a\n"),
        "exit should end the script with its status",
    )?;
    expect(
        shell_on("true; false")? == ran(1, b""),
        "a script should end with the status of its last pipeline",
    )?;
    expect(
        shell_on("echo a\n}")?
            == ran(
                2,
                b"2:1: unexpected-close: remove the }, or open its block before it\n",
            ),
        "a script that does not read should run nothing and say where it is wrong",
    )?;
    expect(
        shell_on("echo a; echo $nope")?
            == ran(
                2,
                b"a\n1:14: unset-variable: set the variable with let before this\n",
            ),
        "a script should stop at a variable never set",
    )
}

/// A function may call itself as deep as the language lets it, 64 calls,
/// on the stack the shell asks for, and one more is an error.
fn functions_call_each_other_deep() -> Outcome {
    let checked = same(
        &shell_on("fn down { down }\ndown")?,
        &ran(2, b"1:11: too-deep: commands run inside each other too deep, as a function calling itself without end does\n"),
    );
    expect(
        checked,
        "a function calling itself should go 64 deep and stop with an error",
    )
}

fn shell_starts_programs() -> Outcome {
    expect(
        shell_on("exitwith; echo $?")? == ran(0, b"exitwith ended with 2989\n255\n"),
        "the shell should start a program by its name, from the loader's on a CD, and keep how it ended in $?",
    )?;
    expect(
        shell_on("nope")? == ran(127, b"nope: no such command\n"),
        "a command that is nowhere should end with 127",
    )?;
    expect(
        shell_on("arguments a 'b c'; echo $?")? == ran(0, b"a\nb c\n0\n"),
        "a program the shell starts should write to the shell's own output",
    )?;
    expect(
        shell_on("/System/Boot/meridian/programs")?.0 == Ok(Ended::Exited(126)),
        "a path that is no program should end with 126",
    )
}

fn shell_writes_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/Sh; echo one > /System/Sh/a; echo two >> /System/Sh/a\n\
         arguments x 'y z' >> /System/Sh/a; cat /System/Sh/a; ls /System/Sh/*\n\
         wc /System/Sh/a /System/Sh/a; rm /System/Sh/a /System/Sh",
    )?;
    if said.starts_with(b"/System/Sh: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(2)),
            "on /System read only the script should stop where nothing matches",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"one\ntwo\nx\ny z\n/System/Sh/a\n\
                     4 5 14 /System/Sh/a\n4 5 14 /System/Sh/a\n8 10 28 total\n",
        "the shell should make a directory, write a file and add to it, from a program too, show it, match it, count it and remove both",
    )?;
    expect(
        open("/System/Sh", abi::file::READ) == Err(Error::NotFound),
        "the directory should be gone",
    )
}

fn shell_matches_patterns() -> Outcome {
    let found = if booted_from_disk() {
        ran(
            0,
            b"/System/Boot/meridian/programs/whoami.mexec /System/Boot/meridian/programs/whoami_again.mexec\n",
        )
    } else {
        ran(
            2,
            b"1:6: no-match: no file matches the pattern; quote it to mean the word itself\n",
        )
    };
    expect(
        shell_on("echo /System/Boot/meridian/programs/wh*")? == found,
        "a pattern should become the paths it matches, in order",
    )?;
    expect(
        shell_on("echo '/System/*' /System/Boot/meridian/programs/whoami.me?ec")?
            == if booted_from_disk() {
                ran(
                    0,
                    b"/System/* /System/Boot/meridian/programs/whoami.mexec\n",
                )
            } else {
                ran(
                    2,
                    b"1:18: no-match: no file matches the pattern; quote it to mean the word itself\n",
                )
            },
        "a quoted pattern should stay the word it is",
    )
}

fn shell_runs_a_file() -> Outcome {
    use abi::file::{CREATE_ALWAYS, WRITE};
    expect(
        shell_with(&["/System/none.sh"])? == ran(2, b"/System/none.sh: no such file\n"),
        "a script that is not there should end the shell with 2",
    )?;
    expect(
        shell_with(&["-c"])?
            == ran(
                2,
                b"shell: start it with no words, -c SCRIPT, or a script's path, after --sh for sh\n",
            ),
        "words the shell does not know should end it with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/script.sh";
    let file = match open_with(path, WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "the script should be made")?,
    };
    let written = write_file(
        file,
        0,
        b"# a script\nfor x in a b {\n  echo $x\n}\nexit 4\n",
    );
    let _ = close(file);
    let ended = shell_with(&[path])?;
    expect(
        written == Ok(()) && remove(path) == Ok(()) && ended == ran(4, b"a\nb\n"),
        "the shell should run the script in a file",
    )
}

fn shell_redirects() -> Outcome {
    expect(
        shell_on("cat < /System/x")? == ran(1, b"/System/x: no such file\n"),
        "a redirection from a file that is not there should say so",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        shell_on("echo a > /System/Boot/x")?
            == ran(1, b"/System/Boot/x: on a volume that is only read\n"),
        "a redirection into a file that cannot be written should say why",
    )?;
    let script = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!("cat < {script} | wc; wc < {script}"))?;
    let (first, second) = said.split_at(said.len() / 2);
    expect(
        ended == Ok(Ended::Exited(0)) && first == second && first.ends_with(b"\n"),
        "< should hand the shell's own commands and programs the file to read",
    )?;
    let (ended, said) = shell_on(
        "echo a > /System/r1 > /System/r2; cat /System/r1 /System/r2; rm /System/r1 /System/r2",
    )?;
    if said.starts_with(b"/System/r1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only nothing should be written or removed",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"a\n",
        "of two redirections the shell should make both files and write into the last",
    )
}

fn shell_pipes() -> Outcome {
    expect(
        shell_on("echo one two | wc")? == ran(0, b"1 2 8\n"),
        "what one command writes the next should read",
    )?;
    expect(
        shell_on("echo x | cat | cat | wc | cat")? == ran(0, b"1 1 2\n"),
        "the shell's own commands should read and write pipes too",
    )?;
    expect(
        shell_on("arguments a 'b c' | wc; echo a | arguments")? == ran(0, b"2 3 6\n"),
        "a program should write into a pipe, and one that reads nothing should end",
    )?;
    expect(
        shell_on("true | false")? == ran(1, b"")
            && shell_on("false | true")? == ran(0, b"")
            && shell_on("nope | wc")? == ran(0, b"nope: no such command\n0 0 0\n"),
        "a pipeline should end as its last command, and one that never started write nothing",
    )?;
    expect(
        shell_on("help | wc")?
            == ran(
                0,
                alloc::format!("2 {HELP_WORDS} {HELP_BYTES}\n").as_bytes(),
            ),
        "a builtin writing more than a message holds should go whole through the pipe",
    )
}

/// Once the shell's input ended with an empty message, reading it again
/// finds the end at once, though nothing closed the end that wrote it.
fn input_that_ended_stays_ended() -> Outcome {
    expect(
        same(
            &shell_reading_to_the_end(&["-c", "cat; cat; echo done"], b"x\n")?,
            &ran(0, b"x\ndone\n"),
        ),
        "a second read after the end should find the end, and not wait for more",
    )
}

/// A command of a pipeline that does not start, or reads what is not
/// there, leaves the others to end as they would.
fn pipes_that_go_wrong() -> Outcome {
    expect(
        shell_on("cat /nope | wc")? == ran(0, b"/nope: no such file\n0 0 0\n"),
        "a command that writes nothing into a pipe should leave the next one an empty stream",
    )?;
    expect(
        shell_on("echo a | cat /nope")? == ran(1, b"/nope: no such file\n"),
        "a pipeline should end as its last command when that went wrong",
    )?;
    expect(
        shell_on("echo a | nope")? == ran(127, b"nope: no such command\n"),
        "a program that is nowhere should end a pipeline with 127",
    )?;
    expect(
        shell_on("echo a | /System/Boot/meridian/programs")?.0 == Ok(Ended::Exited(126)),
        "a path that is no program should end a pipeline with 126",
    )?;
    expect(
        shell_on("exitwith | wc")? == ran(0, b"0 0 0\nexitwith ended with 2989\n"),
        "a program that ends without writing should close its end of the pipe",
    )
}

/// Redirections go before pipes: a command that writes into a file
/// writes nothing into its pipe, and one that reads a file reads nothing
/// from its pipe.
fn pipes_and_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo a b > /System/p1 | wc; echo c d | wc < /System/p1\n\
         wc < /System/p1 | cat > /System/p2 | wc; cat /System/p2 /System/p1\n\
         rm /System/p1 /System/p2",
    )?;
    if said.starts_with(b"/System/p1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made for a pipeline",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"0 0 0\n1 2 4\n0 0 0\n1 2 4\na b\n",
        "a redirection should take the place of the pipe on its side",
    )
}

/// Seven of the shell's own commands and a program, each reading what the
/// one before wrote, all at once.
fn a_long_pipeline() -> Outcome {
    expect(
        shell_on("echo x y | cat | cat | cat | cat | cat | cat | wc")? == ran(0, b"1 2 4\n"),
        "a long pipeline should go through every command",
    )?;
    expect(
        shell_on("echo x | wc | wc | wc")? == ran(0, b"1 3 6\n"),
        "programs one after another should each read what the last wrote",
    )
}

/// The shell's own standard input goes to the first command of a
/// pipeline, and to a command alone, a program or one of the shell's own.
fn shell_hands_its_input_on() -> Outcome {
    let input = b"one two\nthree\n";
    expect(
        shell_reading(&["-c", "wc"], Some(input))? == ran(0, b"2 3 14\n"),
        "a program should read the shell's own input",
    )?;
    expect(
        shell_reading(&["-c", "cat | wc"], Some(input))? == ran(0, b"2 3 14\n"),
        "the first command of a pipeline should read it",
    )?;
    expect(
        shell_reading(&["-c", "cat"], Some(input))? == ran(0, input),
        "cat should show what it reads",
    )?;
    expect(
        shell_reading(&["-c", "echo x | wc; wc"], Some(b"a\n"))? == ran(0, b"1 1 2\n1 1 2\n"),
        "a command after a pipeline should still read the shell's own input",
    )?;
    expect(
        shell_reading(&["-c", "wc; wc"], Some(b"a b\n"))? == ran(0, b"1 2 4\n0 0 0\n"),
        "what one command read the next should not read again",
    )
}

/// A command that reads one file and writes another, the last of two
/// files to read winning, as in sh.
fn reading_and_writing_files_at_once() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo one > /System/f1; echo two three > /System/f2\n\
         cat < /System/f1 > /System/f3; wc < /System/f1 < /System/f2 >> /System/f3\n\
         cat /System/f3; rm /System/f1 /System/f2 /System/f3",
    )?;
    if said.starts_with(b"/System/f1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"one\n1 2 10\n",
        "a command should read one file and write another, and read the last it is given",
    )
}

/// Started with a file as its standard output, the shell writes there,
/// and so do the programs it starts, each after what was there.
fn programs_write_into_the_shells_file() -> Outcome {
    use abi::file::{CREATE_ALWAYS, READ, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/shell.out";
    let file = match open_with(path, WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "the shell's output should be made")?,
    };
    let (child, back) = spawn_handing(
        "shell.mexec",
        &[
            "-c",
            "echo first; arguments a 'b c'; echo x y | wc; echo last",
        ],
        output(start::Sink::File(file)),
    )
    .map_err(|_| "the shell should start")?;
    let ended = wait(child);
    let _ = (close(child), close(back));
    let reader = open(path, READ).map_err(|_| "the shell's output should open")?;
    let mut room = [0u8; 64];
    let len = read(reader, 0, &mut room).unwrap_or(0);
    let _ = close(reader);
    expect(
        ended == Ok(Ended::Exited(0))
            && room[..len] == *b"first\na\nb c\n1 2 4\nlast\n"
            && remove(path) == Ok(()),
        "the shell and its programs should write one after another into the shell's file",
    )
}

/// Bytes that are no text go through files and pipes as they are: cat
/// into a file, a file fed to a program, and a pipe between them.
fn bytes_go_through_as_they_are() -> Outcome {
    use abi::file::{CREATE_ALWAYS, READ, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let bytes = [0xff, 0, 0xc3, b'\n', 0x80, b' ', b'x'];
    let file = match open_with("/System/b1", WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made for the bytes")?,
    };
    let written = write_file(file, 0, &bytes);
    let _ = close(file);
    let (ended, said) =
        shell_on("cat /System/b1 > /System/b2; cat < /System/b1 | wc; wc < /System/b2")?;
    let copy = open("/System/b2", READ).map_err(|_| "the copy should open")?;
    let mut room = [0u8; 16];
    let len = read(copy, 0, &mut room).unwrap_or(0);
    let _ = close(copy);
    let removed = remove("/System/b1") == Ok(()) && remove("/System/b2") == Ok(());
    expect(
        written == Ok(())
            && ended == Ok(Ended::Exited(0))
            && room[..len] == bytes
            && said == b"1 3 7\n1 3 7\n"
            && removed,
        "cat, a redirection and a pipe should pass on bytes that are no text as they are",
    )
}

/// cp and mv carry files where they are told, into a directory by their
/// names, mv a directory too and a file across volumes, and rd removes
/// only empty directories.
fn files_copied_and_moved() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/C; echo one > /System/C/a; cp /System/C/a /System/C/b\n\
         copy /System/C/a /System/C/b /System; mv /System/C/b /System/C/c\n\
         ls /System/C/*; cat /System/a /System/b /System/C/c",
    )?;
    if said.starts_with(b"/System/C: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(2)),
            "on /System read only nothing should be copied or moved",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"/System/C/a\n/System/C/c\none\none\none\n",
        "cp should copy a file to a name and into a directory, and mv move one",
    )?;
    expect(
        shell_on("move /System/C /System/D; ls /System/D/*; rd /System/D/a; rmdir /System/D")?
            == ran(
                1,
                b"/System/D/a\n/System/D/c\n/System/D/a: not a directory\n\
                  /System/D: a directory that holds something\n",
            ),
        "move should carry a directory and what it holds, and rd keep what is no empty directory",
    )?;
    expect(
        shell_on(
            "cp /System/a /System/b /System/x.txt; mv /System/Boot/meridian/boot.meri /System/D",
        )? == ran(
            1,
            b"/System/x.txt: not a directory, so one path alone goes there\n\
              /System/Boot/meridian/boot.meri: on a volume that is only read\n",
        ),
        "two paths should go into no file, and a file across volumes be copied though it stays",
    )?;
    expect(
        shell_on(
            "cat /System/D/boot.meri | wc; wc /System/Boot/meridian/boot.meri\n\
             rm /System/a /System/b /System/D/a /System/D/c /System/D/boot.meri; rd /System/D",
        )?
        .1
        .split(|&b| b == b'\n')
        .take(2)
        .map(|line| line.split(|&b| b == b' ').take(3).collect::<Vec<_>>())
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[0] == w[1]),
        "the copy across volumes should hold what the file holds",
    )?;
    expect(
        open("/System/D", abi::file::READ) == Err(Error::NotFound),
        "rd should remove the empty directory",
    )
}

/// cd changes where paths not from the root start, for the shell's own
/// commands, its patterns and the programs it starts.
fn the_working_directory() -> Outcome {
    expect(
        shell_on("pwd; cd; cd /nope; cd a b")?
            == ran(1, b"/\n/\n/nope: no such file\ncd takes one path\n"),
        "the shell should start where its parent is, and stay when cd goes nowhere",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "cd /System/Boot; pwd; chdir meridian; pwd; wc boot.meri; cat boot.meri | wc\n\
         cd ..; pwd; cd meridian/boot.meri",
    )?;
    let lines: Vec<&[u8]> = said.split(|&b| b == b'\n').collect();
    let counts = |line: &[u8]| {
        line.split(|&b| b == b' ')
            .take(3)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    expect(
        ended == Ok(Ended::Exited(1))
            && lines.len() == 7
            && lines[0] == b"/System/Boot"
            && lines[1] == b"/System/Boot/meridian"
            && lines[2].ends_with(b" boot.meri")
            && counts(lines[2]) == counts(lines[3])
            && lines[4] == b"/System/Boot"
            && lines[5] == b"meridian/boot.meri: not a directory",
        "cd should go into a directory and out, and a program find a file from the shell's",
    )?;
    expect(
        shell_on("cd /System/Boot/meridian/programs; echo wh*; ls wc*")?
            == ran(
                0,
                b"whoami.mexec whoami_again.mexec\nwc.mexec\nwc_again.mexec\n",
            ),
        "patterns not from the root should match in the working directory",
    )
}

/// The root of the tree is a directory, on a CD too, where nothing hangs
/// in it.
fn the_root_is_a_directory() -> Outcome {
    let checked = same(
        &shell_on("test -d /; echo $?; test -f /; echo $?; cd /; pwd")?,
        &ran(0, b"0\n1\n/\n"),
    );
    expect(checked, "test should find the root a directory")
}

/// What a command says went wrong goes to the shell's standard error,
/// and with `2>` and `2>>` into a file instead, for the shell's own
/// commands and its programs alike.
fn errors_go_where_2_sends_them() -> Outcome {
    let (mine, theirs) = pair()?;
    let (errors, error_end) = pair()?;
    let given = start::Given {
        output: Some(start::Sink::Channel(theirs)),
        error: Some(start::Sink::Channel(error_end)),
        ..start::Given::default()
    };
    let (child, back) = spawn_handing(
        "shell.mexec",
        &["-c", "echo a; cat /nope; wc /nope; nope"],
        given,
    )
    .map_err(|_| "the shell should start")?;
    let ended = wait(child);
    let (said, complained) = (drained(mine), drained(errors));
    let _ = (close(child), close(back), close(mine), close(errors));
    expect(
        ended == Ok(Ended::Exited(127))
            && said == b"a\n"
            && complained
                == b"/nope: no such file\nwc: /nope: no such file\nwc ended with 1\nnope: no such command\n",
        "the shell and its programs should say what went wrong on the shell's standard error",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo a 2> /System/e1; cat /nope 2> /System/e1; wc /nope 2>> /System/e1\n\
         cat /System/e1; rm /System/e1",
    )?;
    if said.starts_with(b"/System/e1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should take what went wrong",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"a\nwc ended with 1\n/nope: no such file\nwc: /nope: no such file\n",
        "2> and 2>> should put what went wrong into a file, from a program too",
    )
}

/// `2>&1` sends what goes wrong where the output goes when it comes: into
/// a pipe, into the file a `>` before it named, or where the output went
/// before a `>` after it.
fn errors_go_with_the_output() -> Outcome {
    expect(
        shell_on("cat /nope 2>&1 | wc; wc /nope 2>&1 | wc")?
            == ran(0, b"1 4 20\n1 5 24\nwc ended with 1\n"),
        "2>&1 should send what goes wrong into the pipe",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo hi > /System/o 2>&1; cat /nope >> /System/o 2>&1; wc /nope >> /System/o 2>&1\n\
         cat /System/o; wc /nope 2>&1 > /System/o; cat /System/o; rm /System/o",
    )?;
    if said.starts_with(b"/System/o: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should take the output",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"wc ended with 1\nhi\n/nope: no such file\nwc: /nope: no such file\n\
                     wc: /nope: no such file\nwc ended with 1\n",
        "2>&1 should share a file with the output, and keep where the output went before a > after it",
    )?;
    let (ended, said) = shell_on("cat /nope 2>&1 > /System/o | wc; cat /System/o; rm /System/o")?;
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"1 4 20\n",
        "2>&1 before a > should keep the pipe for what goes wrong while the output goes to the file",
    )
}

/// head, grep and sort, each in a pipeline, and what they say when they
/// are given what they cannot take.
fn head_grep_and_sort() -> Outcome {
    expect(
        shell_on(
            "arguments c a b | sort; arguments c a b | sort -r; arguments one two three | head -n 2",
        )? == ran(0, b"a\nb\nc\nc\nb\na\none\ntwo\n"),
        "sort should put lines in order either way, and head take the first",
    )?;
    expect(
        shell_on(
            "arguments apple Banana cherry | grep an; arguments apple Banana | grep -ic A\n\
                  arguments x y | grep -vn x; arguments x | grep z; echo $?",
        )? == ran(0, b"Banana\n2\n2:y\ngrep ended with 1\n1\n"),
        "grep should find the lines that hold its text, count them, take the others, and end with 1 without any",
    )?;
    expect(
        shell_on("head -n x; grep; sort -x; grep a /nope")?
            == ran(
                2,
                b"head: x is no count of lines\nhead ended with 2\n\
                  grep: needs a pattern to look for\ngrep ended with 2\n\
                  sort: -x is no option\nsort ended with 2\n\
                  grep: /nope: no such file\ngrep ended with 2\n",
            ),
        "head, grep and sort should say what they cannot take and end with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "cd /System/Boot/meridian; head -n 1 boot.meri boot.meri | grep -c ==; head -n 1 boot.meri | wc",
    )?;
    expect(
        ended == Ok(Ended::Exited(0)) && said.starts_with(b"2\n1 "),
        "head should name each of several files, and take one line of one",
    )
}

/// head, grep and sort on files: several at once, named where they say
/// so, a file given with `<`, and one that is not there.
fn tools_on_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) =
        shell_on("echo two > /System/t1; echo one > /System/t2; echo two two >> /System/t2")?;
    if said.starts_with(b"/System/t1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    let checked = shell_on("grep -n two /System/t1 /System/t2; sort /System/t1 /System/t2")?
        == ran(
            0,
            b"/System/t1:1:two\n/System/t2:2:two two\none\ntwo\ntwo two\n",
        )
        && shell_on("head -n 1 /System/t1 /System/t2")?
            == ran(0, b"==> /System/t1 <==\ntwo\n\n==> /System/t2 <==\none\n")
        && shell_on("sort -r < /System/t2; grep -c two < /System/t2")?
            == ran(0, b"two two\none\n1\n")
        && shell_on("grep -c x /System/t1 /nope")?
            == ran(
                2,
                b"/System/t1:0\ngrep: /nope: no such file\ngrep ended with 2\n",
            )
        && shell_on("grep -c x /nope /System/t1")?
            == ran(
                2,
                b"grep: /nope: no such file\n/System/t1:0\ngrep ended with 2\n",
            )
        && shell_on("head /nope; sort /System/t1 /nope")?
            == ran(
                2,
                b"head: /nope: no such file\nhead ended with 1\n\
                  sort: /nope: no such file\nsort ended with 2\n",
            );
    let removed = shell_on("rm /System/t1 /System/t2")? == ran(0, b"");
    expect(
        checked && removed,
        "head, grep and sort should read several files, name them, take one given with <, and say which is not there",
    )
}

/// uniq on a file of 4880 bytes, more than one read of the file and more
/// than one message of a pipe bring: every line comes, however it is
/// given.
fn uniq_on_a_big_file() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    // Tried once first, so that where /System only reads, ten writes do
    // not each take their time to be refused. The line comes again first
    // thing in the loop, and uniq takes it once.
    let x = "x".repeat(480);
    let (ended, said) = shell_on(&alloc::format!("echo line 0 {x} > /System/big"))?;
    if said.starts_with(b"/System/big: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    let made = same(&(ended, said), &ran(0, b""));
    let filled = shell_on(&alloc::format!(
        "for a in 0 1 2 3 4 5 6 7 8 9 {{\necho line $a {x} >> /System/big\n}}"
    ))?;
    let all = b"10 30 4880\n";
    let checked = made
        && same(&filled, &ran(0, b""))
        && same(
            &shell_on(
                "uniq /System/big | wc; cat /System/big | uniq | wc; uniq < /System/big | wc",
            )?,
            &ran(0, &[&all[..], all, all].concat()),
        );
    let removed = shell_on("rm /System/big")? == ran(0, b"");
    expect(
        checked && removed,
        "uniq should take every line of a file longer than one read, from the file, a pipe and <",
    )
}

/// head, grep and sort alone at the shell read what the shell reads.
fn tools_read_the_shells_input() -> Outcome {
    let input = b"pear\napple\nfig\n";
    expect(
        shell_reading(&["-c", "sort"], Some(input))? == ran(0, b"apple\nfig\npear\n")
            && shell_reading(&["-c", "grep -v p"], Some(input))? == ran(0, b"fig\n")
            && shell_reading(&["-c", "head -n 1"], Some(input))? == ran(0, b"pear\n"),
        "the tools should read the shell's input",
    )
}

/// A line longer than a message holds goes through a pipe whole, and a
/// hundred lines come out of sort in order.
fn long_lines_and_many() -> Outcome {
    let long = "x".repeat(300);
    expect(
        shell_on(&alloc::format!(
            "echo {long} | grep x | wc; echo {long}y | grep -c y"
        ))? == ran(0, b"1 1 301\n1\n"),
        "a line of 300 bytes should go through two pipes whole",
    )?;
    // 37 and 100 share no factor, so this is each of 0 to 99 once, out of
    // order.
    let numbers: Vec<String> = (0..100u32)
        .map(|n| alloc::format!("{:03}", (n * 37) % 100))
        .collect();
    let script = alloc::format!("arguments {} | sort", numbers.join(" "));
    let want = (0..100u32).fold(String::new(), |mut want, n| {
        let _ = core::fmt::Write::write_fmt(&mut want, format_args!("{n:03}\n"));
        want
    });
    expect(
        shell_on(&script)? == ran(0, want.as_bytes()),
        "a hundred lines out of order should come out of sort in order",
    )
}

/// tail takes the last lines and uniq each run of a line once, in a
/// pipeline after sort as they are used, and they say what they cannot
/// take.
fn tail_and_uniq() -> Outcome {
    expect(
        shell_on(
            "arguments b a b c a b | sort | uniq; arguments b a b | sort | uniq -c\n\
                  arguments 1 2 3 4 5 | tail -n 2; arguments x y | tail",
        )? == ran(0, b"a\nb\nc\n1 a\n2 b\n4\n5\nx\ny\n"),
        "uniq should take each run once or count it, and tail the last lines",
    )?;
    expect(
        shell_on("tail -n x; uniq /a /b /c; uniq /nope")?
            == ran(
                2,
                b"tail: x is no count of lines\ntail ended with 2\n\
                  uniq: takes an input and an output at most\nuniq ended with 2\n\
                  uniq: /nope: no such file\nuniq ended with 2\n",
            ),
        "tail and uniq should say what they cannot take",
    )
}

/// tail and uniq on files: several at once for tail, named, one given
/// with `<`, and none taken.
fn tail_and_uniq_on_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo a > /System/u1; echo a >> /System/u1; echo b >> /System/u1; echo c > /System/u2",
    )?;
    if said.starts_with(b"/System/u1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    let checked = same(
        &shell_on("tail -n 1 /System/u1 /System/u2; tail -n 0 /System/u1")?,
        &ran(0, b"==> /System/u1 <==\nb\n\n==> /System/u2 <==\nc\n"),
    ) && same(
        &shell_on("uniq -c /System/u1; uniq < /System/u1; tail -n 2 < /System/u1")?,
        &ran(0, b"2 a\n1 b\na\nb\na\nb\n"),
    ) && same(
        &shell_on("tail /System/u1 /nope")?,
        &ran(
            1,
            b"==> /System/u1 <==\na\na\nb\ntail: /nope: no such file\ntail ended with 1\n",
        ),
    );
    let removed = shell_on("rm /System/u1 /System/u2")? == ran(0, b"");
    expect(
        checked && removed,
        "tail and uniq should read files as head and sort do, and say which is not there",
    )
}

/// tail and uniq alone at the shell read what the shell reads.
fn tail_and_uniq_read_the_shells_input() -> Outcome {
    let input = b"a\na\nb\nc\nc\nc\n";
    expect(
        same(
            &shell_reading(&["-c", "uniq -c"], Some(input))?,
            &ran(0, b"2 a\n1 b\n3 c\n"),
        ) && same(
            &shell_reading(&["-c", "tail -n 2"], Some(input))?,
            &ran(0, b"c\nc\n"),
        ),
        "uniq and tail should read the shell's input",
    )
}

/// head and tail go on naming the files after one that is not there,
/// which they say on their standard error.
fn files_named_past_one_not_there() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "cd /System/Boot/meridian; head -n 0 boot.meri /nope boot.meri; tail -n 0 /nope boot.meri",
    )?;
    expect(
        same(
            &(ended, said),
            &ran(
                1,
                b"==> boot.meri <==\nhead: /nope: no such file\n\n==> boot.meri <==\n\
                  head ended with 1\ntail: /nope: no such file\n\n==> boot.meri <==\n\
                  tail ended with 1\n",
            ),
        ),
        "the files after one that is not there should keep their names and the gaps before them",
    )
}

/// The tools one after another as they are used: the most common word of
/// a text, and the last of two hundred lines through a pipe.
fn the_tools_one_after_another() -> Outcome {
    expect(
        same(
            &shell_on(
                "arguments b c a b c b | sort | uniq -c | sort -r | head -n 1\n\
                 arguments a b a | sort | uniq | wc",
            )?,
            &ran(0, b"3 b\n2 2 4\n"),
        ),
        "sort, uniq -c, sort -r and head should find the most common word",
    )?;
    let numbers: Vec<String> = (1..=200u32).map(|n| alloc::format!("{n}")).collect();
    expect(
        same(
            &shell_on(&alloc::format!(
                "arguments {} | tail -n 3",
                numbers.join(" ")
            ))?,
            &ran(0, b"198\n199\n200\n"),
        ),
        "tail should keep the last lines of two hundred that came through a pipe",
    )
}

/// The tools given nothing to read write nothing, and grep ends with 1 as
/// it found nothing; head takes no line when told none.
fn tools_given_nothing() -> Outcome {
    expect(
        shell_on("true | sort; true | head; true | wc; true | grep x; echo $?")?
            == ran(0, b"0 0 0\ngrep ended with 1\n1\n"),
        "nothing read should give nothing written, and grep's 1",
    )?;
    expect(
        shell_on("help | head -n 0 | wc; help | head -n 1 | wc")? == ran(0, b"0 0 0\n1 42 194\n"),
        "head should take no line when told none, and one when told one",
    )
}

/// The shell's own commands write into pipes for the tools, and say what
/// went wrong on the shell's standard error meanwhile.
fn the_shells_own_commands_in_pipes() -> Outcome {
    let listed: &[u8] = if booted_from_disk() {
        b"130\n"
    } else {
        b"/System/Boot/meridian/programs: no such file\n0\ngrep ended with 1\n"
    };
    expect(
        shell_on("ls /System/Boot/meridian/programs | grep -c mexec")?
            == ran(u64::from(!booted_from_disk()), listed),
        "ls should write the programs' names into the pipe, sixty-five twice",
    )?;
    expect(
        same(
            &shell_on("help | grep -c 'or'; pwd | wc; true | wc")?,
            &ran(0, b"2\n1 1 2\n0 0 0\n"),
        ),
        "help, pwd and true should write into a pipe, or not at all",
    )?;
    expect(
        shell_on("cat /nope | cat | wc; services | wc")?
            == ran(
                0,
                b"/nope: no such file\n0 0 0\nservices: init did not start this shell\n0 0 0\n",
            ),
        "the shell's own commands in a pipeline should say what went wrong on the shell's error",
    )
}

/// cp never copies a file onto itself, which would empty it first, even
/// written in other capitals.
fn no_copy_onto_itself() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo kept > /System/s; cp /System/s /System/S; cd /System; copy s ./s\n\
         cat /System/s; rm /System/s",
    )?;
    if said.starts_with(b"/System/s: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only nothing should be copied",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"/System/s: copied onto itself\ns: copied onto itself\nkept\n",
        "cp should refuse to copy a file onto itself and leave what it holds",
    )
}

/// mv to the same name in other capitals renames the file, and grep counts
/// What the shell's own commands say when a path is missing, a directory
/// where a file has to be, or no directory where one has to be, and how
/// they end: each with 1, a file listed alone with 0. A pattern whose last
/// part is a name finds the paths where it is.
fn the_shells_own_commands_say_what_went_wrong() -> Outcome {
    let said = |script: &str, want: &[u8]| -> Result<bool, &'static str> {
        Ok(same(&shell_on(script)?, &ran(0, want)))
    };
    let missing = said(
        "cat /nope; echo $?; ls /nope; echo $?; cat < /nope; echo $?",
        b"/nope: no such file\n1\n/nope: no such file\n1\n/nope: no such file\n1\n",
    )?;
    // A CD brings no /System.
    if !booted_from_disk() {
        return expect(
            missing,
            "cat and ls should say what is wrong with a path and end with 1",
        );
    }
    let checked = missing
        && said(
            "cat /System; echo $?; cat < /System; echo $?",
            b"/System: a directory, not a file\n1\n/System: a directory, not a file\n1\n",
        )?
        && said(
            "ls /System/Boot/meridian/programs/wc.mexec; echo $?",
            b"/System/Boot/meridian/programs/wc.mexec\n0\n",
        )?
        && said(
            "cp /System /nope/x; echo $?; cp /System/Boot/meridian/boot.meri /nope/x; echo $?",
            b"/System: a directory, not a file\n1\n/nope/x: no such file\n1\n",
        )?
        && said(
            "cp a b /System/Boot/meridian/boot.meri; echo $?",
            b"/System/Boot/meridian/boot.meri: not a directory, so one path alone goes there\n1\n",
        )?
        && said(
            "echo /System/Boot/m*/programs",
            b"/System/Boot/meridian/programs\n",
        )?;
    expect(
        checked,
        "cat, ls and cp should say what is wrong with a path and end with 1",
    )
}

/// Starts the shell with `-c text` and an output whose other end is gone
/// before it starts, so that nothing it writes there arrives; what it says
/// went wrong comes on a channel of its own.
fn shell_into_nothing(text: &str) -> Result<Ran, &'static str> {
    let (gone, theirs) = pair()?;
    expect(close(gone) == Ok(()), "the output's other end should close")?;
    let (mine, errors) = pair()?;
    let mut given = output(start::Sink::Channel(theirs));
    given.error = Some(start::Sink::Channel(errors));
    let (child, back) =
        spawn_handing("shell.mexec", &["-c", text], given).map_err(|_| "the shell should start")?;
    let ended = wait(child);
    let said = drained(mine);
    expect(
        close(child) == Ok(()) && close(back) == Ok(()) && close(mine) == Ok(()),
        "the shell's handles should close",
    )?;
    Ok((ended, said))
}

/// cat, cat with `<` and ls end with 1 when what they write goes nowhere,
/// and say nothing, as there is nowhere to say it.
fn the_shell_ends_with_1_when_its_output_is_gone() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let mut all = true;
    for script in [
        "cat /System/Boot/meridian/boot.meri",
        "cat < /System/Boot/meridian/boot.meri",
        "ls /System/Boot/meridian",
    ] {
        all &= same(&shell_into_nothing(script)?, &ran(1, b""));
    }
    expect(
        all,
        "cat and ls should end with 1 when their output is gone",
    )
}

/// A file that ends in the middle of a character shows the rest of it as
/// a character that is not there, read by cat from the file or through
/// `<`.
fn a_character_cut_short_shows() -> Outcome {
    use abi::file::{CREATE_ALWAYS, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/cut.txt";
    let file = match open_with(path, WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made on /System")?,
    };
    let written = write_file(file, 0, b"a\xc3") == Ok(()) && close(file) == Ok(());
    let shown = same(
        &shell_on("cat /System/cut.txt; cat < /System/cut.txt")?,
        &ran(0, b"a\xef\xbf\xbda\xef\xbf\xbd"),
    );
    expect(
        written && shown && remove(path) == Ok(()),
        "a character a file cuts short should show as one that is not there",
    )
}

/// The files a command opened for its redirections close when a later one
/// cannot open, alone and in a pipeline, so the file can go right after.
fn files_close_when_a_command_cannot_start() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on("echo f > /System/f")?;
    if said.starts_with(b"/System/f: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    let alone = same(
        &shell_on("cat < /System/f > /nope/x; rm /System/f; echo $?")?,
        &ran(0, b"/nope/x: no such file\n0\n"),
    );
    let made = same(&shell_on("echo f > /System/f")?, &ran(0, b""));
    let piped = same(
        &shell_on("cat < /System/f | wc > /nope/x; rm /System/f; echo $?")?,
        &ran(0, b"/nope/x: no such file\n0\n"),
    );
    expect(
        same(&(ended, said), &ran(0, b"")) && alone && made && piped,
        "a file a command opened should close when the command cannot start",
    )
}

/// echo, printf, test and `[` run in the shell as lib/shell has them, with
/// their statuses, in a pipeline too, and what goes wrong said before what
/// printf writes at its end. Where the shell's output is a channel, it is
/// no terminal.
fn the_shared_commands() -> Outcome {
    let checked = same(
        &shell_on(
            "printf '%s-%03d|' a 7; echo -n x; echo y; test 1 -lt 2; echo $?; [ a = b ]; echo $?",
        )?,
        &ran(0, b"a-007|xy\n0\n1\n"),
    ) && same(
        &shell_on("test; echo $?; [ x; echo $?; printf '%d\\n' 9x; echo $?")?,
        &ran(
            0,
            b"1\n[: needs a ] at its end\n2\nprintf: 9x: not all a number\n9\n1\n",
        ),
    ) && same(
        &shell_on("printf 'p\\n' | wc; test -t 1; echo $?")?,
        &ran(0, b"1 1 2\n1\n"),
    );
    expect(
        checked,
        "echo, printf, test and [ should run as lib/shell has them, with their statuses",
    )
}

/// `test -t` sees a stream as a terminal only where the command has the
/// shell's own and the shell has the console's: not one handed to the
/// shell, not one redirected, and none past 2.
fn streams_that_are_terminals() -> Outcome {
    let checked = same(
        &shell_on(
            "test -t 0; echo $?; test -t 1; echo $?; test -t 2; echo $?; test -t 5; echo $?",
        )?,
        &ran(0, b"0\n1\n1\n1\n"),
    ) && same(
        &shell_on("#!/bin/sh\ntest -t 0 <<EOF\nx\nEOF\necho $?")?,
        &ran(0, b"1\n"),
    ) && same(
        &shell_reading(&["-c", "test -t 0; echo $?"], Some(b""))?,
        &ran(0, b"1\n"),
    );
    expect(
        checked,
        "test -t should see only the console the shell was not handed another for",
    )
}

/// test asks the files on /System what they are and what may be done with
/// them: a program's image may be run, whatever the capitals of its name,
/// and one may write what the volume lets be written.
fn test_asks_the_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let writable = open(boot, abi::file::WRITE).is_ok_and(|handle| close(handle) == Ok(()));
    let programs = "/System/Boot/meridian/programs";
    let what = same(
        &shell_on(&alloc::format!(
            "test -d /System; echo $?; test -f {boot}; echo $?; test -s {boot}; echo $?; \
             test -e /nope; echo $?; test -r {boot}; echo $?"
        ))?,
        &ran(0, b"0\n0\n0\n1\n0\n"),
    );
    let allowed = same(
        &shell_on(&alloc::format!(
            "test -x {programs}/wc.mexec; echo $?; test -x {programs}/WC.MEXEC; echo $?; \
             test -x {boot}; echo $?; test -w {boot}; echo $?"
        ))?,
        &ran(
            0,
            if writable {
                b"0\n0\n1\n0\n"
            } else {
                b"0\n0\n1\n1\n"
            },
        ),
    );
    expect(
        what && allowed,
        "test should ask the files what they are and what may be done with them",
    )
}

/// A script whose first line names sh runs as sh: its functions, loops
/// and `case`, a `$(...)` around a pipeline with a program in it, and the
/// status `exit` gives.
fn the_sh_dialect() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             f() { echo \"f:$1\"; }\n\
             for i in a b; do f $i; done\n\
             x=$(echo cap | wc); echo \"[$x]\"\n\
             case $x in *4) echo four;; esac\n\
             g() { local v=in; echo $v; }; v=out; g; echo $v\n\
             exit 3",
        )?,
        &ran(3, b"f:a\nf:b\n[1 1 4]\nfour\nin\nout\n"),
    );
    expect(
        checked,
        "a script whose first line names sh should run as sh",
    )
}

/// In sh, the redirections of a compound command hold for all inside it,
/// `2>&1` takes stream 1 as it is then, a capture too, a closed stream
/// takes nothing, and a stream past 2 is refused.
fn sh_streams() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             { echo a; echo b >&2; } 2>&-\n\
             x=$( { echo c; echo d >&2; } 2>&1 ); echo \"[$x]\"\n\
             echo e >&-; echo \"closed $?\"\n\
             { echo f >&2; } 2>&1 >&-\n\
             echo g 3>&1; echo \"three $?\"",
        )?,
        &ran(
            0,
            b"a\n[c\nd]\nclosed 1\nf\n\
              3: Meridian's programs have the streams 0, 1 and 2\nthree 1\n",
        ),
    );
    expect(
        checked,
        "sh's redirections should hold for a whole block, copy and close streams",
    )
}

/// In sh, what goes wrong in a block goes where the block's stream 2
/// does: what a command says itself, that a command is not there, how a
/// program ended, and that a redirection could not be made.
fn sh_errors_follow_stream_2() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             { cat /nope; nope; cat < /nope; wc /nope; } 2>&-\n\
             x=$( { cat /nope; nope; wc /nope; } 2>&1 ); echo \"[$x]\"\n\
             { cat < /nope; } 2>&-; echo \"status $?\"",
        )?,
        &ran(
            0,
            b"[/nope: no such file\nnope: no such command\n\
              wc: /nope: no such file\nwc ended with 1]\nstatus 1\n",
        ),
    );
    expect(
        checked,
        "what goes wrong in a block should go where the block's stream 2 goes",
    )
}

/// In sh, a here-document is the text its command reads, a program's or
/// the shell's own, expanded unless its word is quoted, and one on a
/// compound command is read inside it.
fn sh_here_documents() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             v=two\n\
             wc <<EOF\n\
             one $v\n\
             EOF\n\
             wc <<'EOF' | wc\n\
             x $v y\n\
             EOF\n\
             cat <<EOF\n\
             three\n\
             EOF\n\
             { wc; } <<EOF\n\
             in a frame\n\
             EOF\n",
        )?,
        &ran(0, b"1 2 8\n1 3 6\nthree\n1 3 11\n"),
    );
    expect(
        checked,
        "a here-document should be what programs and the shell's commands read",
    )
}

/// In sh, the `cd` of a subshell stays in it, `$(...)` runs in a copy of
/// the session, export says what it marked, and set sets the parameters.
fn sh_session() -> Outcome {
    let inside = if booted_from_disk() { "/System" } else { "/" };
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             cd /\n\
             (cd /System 2>&-; pwd); pwd\n\
             a=1; b=$(a=2; echo $a); echo \"$a$b\"\n\
             export e=1; export -p\n\
             set -- x 'y z'; echo $# \"$2\"",
        )?,
        &ran(
            0,
            alloc::format!("{inside}\n/\n12\nexport e='1'\n2 y z\n").as_bytes(),
        ),
    );
    expect(
        checked,
        "a subshell should keep its cd, and export and set should do their part",
    )
}

/// In sh, what does not read stops the script before it runs, an error
/// in it stops it where it is, both with 2, and `${name?}`, a special
/// builtin's trouble and `set -e` stop it with 1.
fn sh_errors() -> Outcome {
    for (script, status, said) in [
        (
            "#!sh\nif true; then",
            2,
            &b"2:10: missing-command: put a command here\n"[..],
        ),
        (
            "#!/bin/sh\necho a\necho ${a;b}\necho c",
            2,
            b"a\n3:6: bad-substitution: write ${name}, or ${name} with an operator such as :- after it\n",
        ),
        ("#!/bin/sh\necho ${u?gone}; echo no", 1, b"u: gone\n"),
        ("#!/bin/sh\nset -e\necho a\nfalse\necho b", 1, b"a\n"),
        (
            "#!/bin/sh\nset -x\necho a | wc",
            0,
            b"+ echo a\n+ wc\n1 1 2\n",
        ),
        (
            "#!/bin/sh\nreadonly r=1; r=2",
            2,
            b"2:15: read-only: this variable is read only, so it keeps its value\n",
        ),
        (
            "#!/bin/sh\nf() { f; }; f",
            2,
            b"2:7: too-deep: commands run inside each other too deep, as a function calling itself without end does\n",
        ),
        (
            "#!/bin/sh\nshift; echo no",
            1,
            b"shift: 1: more than the 0 parameters there are\n",
        ),
    ] {
        expect(
            same(&shell_on(script)?, &ran(status, said)),
            "a sh script should stop as POSIX has it, saying why",
        )?;
    }
    Ok(())
}

/// In sh, `set -C` makes `>` refuse a file that is there while `>|`
/// writes it, and `<>` opens one to read and write.
fn sh_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "#!/bin/sh\n\
         set -C\n\
         echo x > /System/c1 || exit 9\n\
         echo y > /System/c1; echo \"again $?\"\n\
         echo z >| /System/c1; wc <> /System/c1; rm /System/c1",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said == b"/System/c1: on a volume that is only read\n",
            "on /System read only the first file should not be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"/System/c1: there already\nagain 1\n1 1 2\n",
        "set -C should keep a file from being written over, but for >|",
    )?;
    expect(
        open("/System/c1", abi::file::READ) == Err(Error::NotFound),
        "the file should be gone",
    )
}

/// In sh, a pipeline with a function or a compound command in it runs
/// its commands in their order, what one writes the next one's input, a
/// program's among them, and inside a `$(...)` too.
fn sh_pipelines() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             f() { echo x; echo y; }\n\
             f | wc\n\
             echo one two | f | wc\n\
             x=$(f | wc); echo \"[$x]\"\n\
             f | { wc; echo end; }",
        )?,
        &ran(0, b"2 2 4\n2 2 4\n[2 2 4]\n2 2 4\nend\n"),
    );
    expect(
        checked,
        "a pipeline with a function in it should hand on what each part writes",
    )
}

/// In sh, a `$(...)` takes more than a channel holds while its commands
/// run, and one inside another takes what is written inside it.
fn sh_captures() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             x=$(for i in 1 2 3 4 5 6 7 8 9 10 11 12; do echo $i; done)\n\
             echo \"$x\" | wc\n\
             y=$(echo a; z=$(echo b | wc); echo \"$z\"); echo \"[$y]\"\n\
             x=$(exit 4); echo \"[$x] $?\"",
        )?,
        &ran(0, b"12 12 27\n[a\n1 1 2]\n[] 4\n"),
    );
    expect(
        checked,
        "a capture should take all its commands write, and one inside another its own",
    )
}

/// In sh, a function's own redirections hold inside those of its call, and
/// the call's take its streams as they are.
fn sh_function_streams() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             f() { echo in; } >&2\n\
             f; f 2>&-; echo \"closed $?\"\n\
             g() { echo $1 >&2; wc; }\n\
             g a 2>&1 <<EOF\n\
             one two\n\
             EOF\n",
        )?,
        &ran(0, b"in\nclosed 1\na\n1 2 8\n"),
    );
    expect(
        checked,
        "a function's redirections should hold inside those its call makes",
    )
}

/// In sh, `$0` names the script, a `-c` one the shell, which hands it no
/// parameters; `set` gives it some, the shell's commands and tests run in
/// it as in Meridian's dialect, and `getopts` takes options from them.
fn sh_parameters() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             echo $0 $# $$\n\
             set -- a 'b c'; for p; do echo \"[$p]\"; done\n\
             shift; echo $1\n\
             [ \"$1\" = \"b c\" ] && test -n \"$1\" && echo both\n\
             set -- -ab v w; while getopts ab: o; do echo \"$o$OPTARG\"; done; echo $OPTIND",
        )?,
        &ran(0, b"shell 0 0\n[a]\n[b c]\nb c\nboth\na\nbv\n3\n"),
    );
    expect(
        checked,
        "a sh script should have its name and the parameters set gives it",
    )
}

/// A script in a file whose first line names sh runs as sh, with its path
/// as `$0`.
fn sh_script_files() -> Outcome {
    use abi::file::{CREATE_ALWAYS, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/script-sh.sh";
    let file = match open_with(path, WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "the script should be made")?,
    };
    let written = write_file(
        file,
        0,
        b"#!/bin/sh\nfor x in a b; do echo \"$0 $x\"; done\nexit $#\n",
    );
    let _ = close(file);
    let ended = shell_with(&[path])?;
    expect(
        written == Ok(())
            && remove(path) == Ok(())
            && same(
                &ended,
                &ran(0, b"/System/script-sh.sh a\n/System/script-sh.sh b\n"),
            ),
        "the shell should run a sh script in a file, its path as $0",
    )
}

/// In sh, a word that is a pattern stands for the names of the files it
/// matches, in the order of their bytes, capitals as other letters, and
/// stays as it is when it matches none or `set -f` is on.
fn sh_names_of_files() -> Outcome {
    let root = if booted_from_disk() { "/System" } else { "/S*" };
    let checked = same(
        &shell_on("#!/bin/sh\necho /S*")?,
        &ran(0, alloc::format!("{root}\n").as_bytes()),
    );
    expect(checked, "a pattern should match the places in the root")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             cd /System/Boot/meridian/programs\n\
             echo w*\n\
             echo /System/Boot/meridian/programs/s?ell.mexec\n\
             for f in [hi]*_again.mexec; do echo \"$f\"; done\n\
             echo W* .x*\n\
             set -f; echo w*",
        )?,
        &ran(
            0,
            b"wc.mexec wc_again.mexec whoami.mexec whoami_again.mexec\n\
              /System/Boot/meridian/programs/shell.mexec\n\
              head_again.mexec\nhello_again.mexec\ninit_again.mexec\n\
              W* .x*\nw*\n",
        ),
    );
    expect(
        checked,
        "a pattern should stand for the names of the files it matches",
    )
}

/// In sh, `read` takes a line from the stream 0 in force into its
/// variables, and what reads that stream after starts past the line: a
/// here-document, a pipe, the shell's own input, and a file.
fn sh_read() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             while IFS=: read k v; do echo \"$k=$v\"; done <<EOF\n\
             a:1\n\
             b:2:3\n\
             EOF\n\
             { read a; read b; cat; } <<EOF\n\
             1\n\
             2\n\
             3\n\
             EOF\n\
             echo \"$b$a\"\n\
             printf 'p q\\n' | { read x y; echo \"$y $x\"; }",
        )?,
        &ran(0, b"a=1\nb=2:3\n3\n21\nq p\n"),
    );
    expect(checked, "read should take lines from what stream 0 reads")?;
    let checked = same(
        &shell_reading(
            &[
                "-c",
                "#!/bin/sh\nread a b; echo \"$b $a\"; read c; echo \"[$c]\"",
            ],
            Some(b"x y\nz\n"),
        )?,
        &ran(0, b"y x\n[z]\n"),
    );
    expect(checked, "read should take lines from the shell's own input")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "#!/bin/sh\n\
         f=/System/Boot/meridian/boot.meri\n\
         wc < $f; { read first; wc; } < $f",
    )?;
    let counts: Vec<u64> = core::str::from_utf8(&said)
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let fewer = match counts[..] {
        [lines, _, _, after, _, _] => lines == after + 1,
        _ => false,
    };
    expect(
        ended == Ok(Ended::Exited(0)) && fewer,
        "a program reading a file after read should start past the line",
    )
}

/// In sh, a trap on `EXIT` runs as the script ends, its status kept, and
/// a trap on a signal is refused, as Meridian sends none.
fn sh_trap() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             trap 'echo bye $?' EXIT\n\
             trap : INT\n\
             (trap 'echo sub' EXIT; echo in)\n\
             exit 3",
        )?,
        &ran(3, b"trap: INT: Meridian sends no signals\nin\nsub\nbye 3\n"),
    );
    expect(checked, "a trap on EXIT should run as the script ends")
}

/// In sh, `.` runs the commands of a file in the session itself, and a
/// file it cannot read ends the script.
fn sh_dot() -> Outcome {
    let checked = same(
        &shell_on("#!/bin/sh\n. /System/nope.sh; echo no")?,
        &ran(1, b".: /System/nope.sh: no such file\n"),
    );
    expect(
        checked,
        ". should end the script on a file that is not there",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "#!/bin/sh\n\
         echo 'v=sourced; echo in $v' > /System/dot.sh || exit 9\n\
         . /System/dot.sh\n\
         echo \"$v\"\n\
         rm /System/dot.sh",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said == b"/System/dot.sh: on a volume that is only read\n",
            "on /System read only the file should not be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"in sourced\nsourced\n",
        ". should run a file's commands in the session itself",
    )
}

/// In sh, `cd` goes to `$HOME`, which the shell sets to /System, without a
/// word, and back with `-`, keeping `PWD` and `OLDPWD`.
fn sh_cd() -> Outcome {
    let want: &[u8] = if booted_from_disk() {
        b"/System\n/\n/ /System\ncd: /nope: no such file\n1\n"
    } else {
        b"cd: /System: no such file\n/\n/\n/ /\ncd: /nope: no such file\n1\n"
    };
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             cd /; cd; pwd\n\
             cd -; echo \"$PWD $OLDPWD\"\n\
             cd /nope; echo $?",
        )?,
        &ran(0, want),
    );
    expect(
        checked,
        "cd should go home, back, and say where it cannot go",
    )
}

/// In sh, `command -v` finds the shell's own commands and, on a disk, a
/// program's file, and `command` runs a command whose name a function has.
fn sh_command() -> Outcome {
    let want: &[u8] = if booted_from_disk() {
        b"/System/Boot/meridian/programs/wc.mexec\nls\n1\nreal\n"
    } else {
        b"ls\n1\nreal\n"
    };
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             command -v wc ls nope; echo $?\n\
             echo() { printf 'fake\\n'; }; command echo real",
        )?,
        &ran(0, want),
    );
    expect(
        checked,
        "command should find commands and run them past functions",
    )
}

/// In sh, `command -v` names the shell program's own commands, and finds
/// a program by its path when its file is there; it ends with 1 for a name
/// or a path no program has.
fn sh_command_paths() -> Outcome {
    let wc = "/System/Boot/meridian/programs/wc.mexec";
    let want = if booted_from_disk() {
        alloc::format!("cat\nls\n{wc}\n0\n1\n")
    } else {
        String::from("cat\nls\n1\n1\n")
    };
    let checked = same(
        &shell_on(&alloc::format!(
            "#!/bin/sh\ncommand -v cat ls {wc}; echo $?\n\
             command -v nope /System/Boot/meridian/programs/nope.mexec; echo $?"
        ))?,
        &ran(0, want.as_bytes()),
    );
    expect(
        checked,
        "command -v should name the shell's commands and find a program by its path",
    )
}

/// `sh` in Meridian's dialect starts the shell in the sh dialect: with
/// `-c` on a script that names no dialect, and Meridian's shell says how
/// it ended.
fn sh_started() -> Outcome {
    let checked = same(
        &shell_on("sh -c 'x=1; echo $((x + 1)) $0'")?,
        &ran(0, b"2 shell\n"),
    );
    expect(checked, "sh -c should run its script in the sh dialect")?;
    let checked = same(&shell_on("sh -c 'exit 4'")?, &ran(4, b"sh ended with 4\n"));
    expect(checked, "the shell should say how sh ended")
}

/// In sh, `exec` makes its redirections the shell's own from then on: in
/// a block that set the stream, until the block ends, and in a subshell or
/// a `$(...)`, only there. One that cannot redirect ends the script.
fn sh_exec() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             { exec 2>&1; nope; } 2>&-\n\
             (exec >&-; echo lost); echo kept\n\
             x=$(exec 2>&1; nope); echo \"[$x]\"\n\
             exec 2>&-; nope; echo end",
        )?,
        &ran(
            0,
            b"nope: no such command\nkept\n[nope: no such command]\nend\n",
        ),
    );
    expect(checked, "exec should make its redirections the shell's own")?;
    let checked = same(
        &shell_on("#!/bin/sh\nexec > /nope/x\necho still")?,
        &ran(1, b"/nope/x: no such file\n"),
    );
    expect(
        checked,
        "an exec whose redirection fails should end the script with 1",
    )
}

/// In sh, `&` starts what the shell runs whole as a job, its stream 0
/// closed, `$!` names it, and `wait` waits for it and ends as it ended.
fn sh_jobs() -> Outcome {
    let checked = same(
        &shell_on(
            "#!/bin/sh\n\
             wc & wait $!; echo \"waited $? ${!:+job}\"\n\
             nope & wait $!; echo $?\n\
             wait $!; echo $?\n\
             echo x | wc &\n\
             wait $!; echo \"pipe $? $!\"",
        )?,
        &ran(
            0,
            b"0 0 0\nwaited 0 job\nnope: no such command\n127\n\
              wait: 2: no such job\n127\n\
              1 1 2\npipe 0 3\n",
        ),
    );
    expect(checked, "& should start a job, and wait wait for it")
}

/// In sh, two jobs run side by side, each reading what a here-document
/// gives its stream 0, in whichever order they end, and `wait` alone waits
/// for both.
fn sh_jobs_side_by_side() -> Outcome {
    let (ended, said) = shell_on(
        "#!/bin/sh\n\
         wc <<EOF &\n\
         one\n\
         EOF\n\
         wc <<EOF &\n\
         two words\n\
         EOF\n\
         wait; echo \"all $?\"",
    )?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let mut lines: Vec<&str> = text.lines().collect();
    let last = lines.pop();
    lines.sort_unstable();
    expect(
        ended == Ok(Ended::Exited(0)) && last == Some("all 0") && lines == ["1 1 4", "1 2 10"],
        "two jobs should each read their here-document, and wait wait for both",
    )
}

/// In sh, `exec` sends what a subshell writes into a file from then on,
/// and the shell writes where it did after the subshell.
fn sh_exec_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "#!/bin/sh\n\
         : > /System/exec.txt || exit 9\n\
         (exec > /System/exec.txt; echo inside; echo also)\n\
         echo outside; cat /System/exec.txt; rm /System/exec.txt",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said == b"/System/exec.txt: on a volume that is only read\n",
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"outside\ninside\nalso\n",
        "exec should send a subshell's output into the file from then on",
    )
}

/// In sh, `read` takes a file's first line through `exec <`, the line
/// `head` shows, and a command reading the file after it starts past the
/// lines read.
fn sh_read_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "#!/bin/sh\n\
         f=/System/Boot/meridian/boot.meri\n\
         head -n 2 $f\n\
         exec < $f; IFS= read -r a; IFS= read -r b; printf '%s\\n%s\\n' \"$a\" \"$b\"",
    )?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    expect(
        ended == Ok(Ended::Exited(0)) && lines.len() == 4 && lines[..2] == lines[2..],
        "read should take the lines of a file exec gives stream 0",
    )
}

/// in each of several files by its name.
fn a_move_to_other_capitals() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo one > /System/qq; mv /System/qq /System/QQ; ls /System/Q*; cat /System/qq\n\
         grep -c o /System/QQ /System/qq /System/Qq; rm /System/QQ",
    )?;
    if said.starts_with(b"/System/qq: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(2)),
            "on /System read only nothing should be renamed, and then nothing match",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"/System/QQ\none\n/System/QQ:1\n/System/qq:1\n/System/Qq:1\n",
        "mv should rename a file to other capitals, which a path in any capitals then finds",
    )
}

/// The tools say a directory is no file, and pass lines that are no text
/// on as they are.
fn tools_refuse_what_is_no_text() -> Outcome {
    use abi::file::{CREATE_ALWAYS, WRITE};
    let directory: &[u8] = if booted_from_disk() {
        b"grep: /System/Boot: a directory, not a file\ngrep ended with 2\n\
          head: /System/Boot: a directory, not a file\nhead ended with 1\n"
    } else {
        b"grep: /System/Boot: no such file\ngrep ended with 2\n\
          head: /System/Boot: no such file\nhead ended with 1\n"
    };
    expect(
        shell_on("grep x /System/Boot; head /System/Boot")? == ran(1, directory),
        "a directory should be no file to read",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let file = match open_with("/System/raw", WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made for the bytes")?,
    };
    let written = write_file(file, 0, &[0xff, b'x', b'\n', b'y', 0xfe, b'\n']);
    let _ = close(file);
    let ran_it = shell_on("grep x /System/raw; sort -r /System/raw; head -n 1 /System/raw")?;
    let removed = remove("/System/raw") == Ok(());
    expect(
        written == Ok(())
            && removed
            && ran_it
                == ran(
                    0,
                    &[
                        0xff, b'x', b'\n', 0xff, b'x', b'\n', b'y', 0xfe, b'\n', 0xff, b'x', b'\n',
                    ],
                ),
        "grep, sort and head should pass on lines that are no text as they are",
    )
}

/// grep takes basic expressions, extended ones with -E and texts as they
/// are with -F, and whole lines with -x; -q says nothing and ends with 0
/// once it took a line, -l names the files that hold one, -s keeps quiet
/// about a file it cannot read, and a pattern that is no expression ends
/// it with 2.
fn grep_expressions() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'a.c\\nabc\\naac\\n' | grep -c a.c\n\
             printf 'a.c\\nabc\\n' | grep -F a.c\n\
             printf 'aa\\nab\\n' | grep -E '^a+$'\n\
             printf 'ab\\nabab\\nabx\\n' | grep -xc 'ab*'\n\
             printf 'ab\\nabab\\n' | grep -x '\\(ab\\)*'",
        )?,
        &ran(0, b"3\na.c\naa\n1\nab\nabab\n"),
    );
    expect(
        checked,
        "grep should match basic and extended expressions, texts as they are and whole lines",
    )?;
    let checked = same(
        &shell_on(
            "printf 'x\\n' | grep -q x; echo $?; printf 'x\\n' | grep -l x; grep 'a\\{2'; echo $?\n\
             printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n' | grep '\\(\\(a*\\)*\\)*\\1x'; echo $?",
        )?,
        &ran(
            0,
            b"0\n(standard input)\n\
              grep: a\\{2: an interval that is no count from 0 to 255\ngrep ended with 2\n2\n\
              grep: -: the back-references leave too many ways to match this text\n\
              grep ended with 2\n2\n",
        ),
    );
    expect(
        checked,
        "grep -q and -l should say only what they are for, and grep refuse what is no expression \
         or too complex to search",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let checked = same(
        &shell_on(&alloc::format!(
            "grep -l = {boot} /nope; grep -ls = /nope {boot}; echo $?\n\
             grep -q = {boot} /nope; echo $?; grep -q zz {boot} /nope; echo $?"
        ))?,
        &ran(
            0,
            alloc::format!(
                "{boot}\ngrep: /nope: no such file\ngrep ended with 2\n{boot}\ngrep ended with 2\n2\n\
                 0\ngrep: /nope: no such file\ngrep ended with 2\n2\n"
            )
            .as_bytes(),
        ),
    );
    expect(
        checked,
        "grep -l should name the files that hold a line, -s keep quiet about one it cannot read, \
         and -q end at the first file that holds one",
    )
}

/// sed runs a script on its input: s in basic and extended expressions,
/// parts given with -e, $= on the last line, N at the end writing nothing
/// as POSIX has it, and a last line without its line feed.
fn sed_on_its_input() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'a\\nb\\n' | sed s/a/x/\n\
             printf '1\\n2\\n3\\n' | sed -n '$='\n\
             printf 'a\\nb\\nc\\n' | sed -e 1d -e s/b/B/\n\
             printf '1\\n2\\n3\\n' | sed 'N;s/\\n/+/'\n\
             echo aaa | sed -E 's/a+/x/'\n\
             printf 'a\\nb' | sed 's/$/!/'",
        )?,
        &ran(0, b"x\nb\n3\nB\nc\n1+2\nx\na!\nb!\n"),
    );
    expect(checked, "sed should run its script on each line it reads")?;
    let checked = same(
        &shell_on("sed 'p;k'; echo $?; sed -x p")?,
        &ran(
            1,
            b"sed: 1:3: k is no command\nsed ended with 1\n1\n\
              sed: -x is no option\nsed ended with 1\n",
        ),
    ) && same(
        &shell_on("printf 'x\\n' | sed -n p /nope; echo $?")?,
        &ran(0, b"sed: /nope: no such file\nsed ended with 2\n2\n"),
    );
    expect(
        checked,
        "sed should say where its script is wrong, and end with 2 for a file it cannot read",
    )
}

/// sed reads a whole script from a file with -f, however long, writes
/// lines into a file with w and a file into its output with r, and reads
/// several files as one input.
fn sed_on_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    // Longer than the pieces a file is read in, so a script read only in
    // part would lose the command at its end.
    let script = alloc::format!("{}s/a/b/", "s/x/x/;".repeat(20));
    let (ended, said) = shell_on(&alloc::format!(
        "echo '{script}' > /System/s.sed || exit 9\n\
         echo a | sed -f /System/s.sed\n\
         printf 'a\\nb\\n' | sed -n '/b/w /System/sed.txt'; cat /System/sed.txt\n\
         printf 'x\\n' | sed 'r /System/s.sed'\n\
         rm /System/sed.txt /System/s.sed",
    ))?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/s.sed: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == alloc::format!("b\nb\nx\n{script}\n").as_bytes(),
        "sed should take a whole script from a file, write lines into one and read one in",
    )?;
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "sed -n '$=' {boot} {boot}; cat {boot} {boot} | sed -n '$='"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let counts: Vec<&str> = text.lines().collect();
    expect(
        ended == Ok(Ended::Exited(0))
            && counts.len() == 2
            && counts[0] == counts[1]
            && counts[0].parse::<usize>().is_ok_and(|count| count > 1),
        "sed should read two files as one input, its last line the second's",
    )?;
    // A directory opens but gives no bytes: sed says so, names it, and
    // ends with 2.
    let checked = same(
        &shell_on("sed p /System/Boot; echo $?")?,
        &ran(
            0,
            b"sed: /System/Boot: a directory, not a file\nsed ended with 2\n2\n",
        ),
    );
    expect(
        checked,
        "sed should say which input it could not read and end with 2",
    )
}

/// sed writes what it makes in pieces as long as a message, and a text
/// far longer than one comes out whole: each line printed twice is as
/// many lines and bytes as the text twice.
fn sed_on_a_long_text() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let sixteen = [boot; 16].join(" ");
    let thirty_two = [boot; 32].join(" ");
    let (ended, said) = shell_on(&alloc::format!(
        "cat {sixteen} | sed p | wc; cat {thirty_two} | wc; cat {sixteen} | sed -n '$='; cat {sixteen} | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    let counted = |line: &str| line.split(' ').next().and_then(|n| n.parse::<usize>().ok());
    expect(
        ended == Ok(Ended::Exited(0))
            && lines.len() == 4
            && lines[0] == lines[1]
            && counted(lines[1]).is_some_and(|n| n > 32)
            && lines[2].parse::<usize>().ok() == counted(lines[3]),
        "sed should write a text longer than a message whole, and count its lines",
    )
}

/// What sed the program adds to lib/sed: an a\ text that goes on in the
/// next -e, q ending a pipe's input early, l, y, = and G as written out,
/// a w file that cannot be made, which ends sed with 4, a search that
/// would take too long, and where a script read from a file is wrong.
fn sed_as_a_program() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'x\\n' | sed -e 'a\\' -e 'after'\n\
             printf '1\\n2\\n3\\n' | sed 1q\n\
             printf 'a\\tb\\n' | sed -n l\n\
             printf 'ab\\nc\\n' | sed 'y/abc/xyz/;=;G'",
        )?,
        &ran(0, b"x\nafter\n1\na\\tb$\n1\nxy\n\n2\nz\n\n"),
    );
    expect(
        checked,
        "sed should join its -e parts, end early at q, and write l, y, = and G",
    )?;
    let checked = same(
        &shell_on("printf 'x\\n' | sed 'w /nope/w'; echo $?")?,
        &ran(0, b"sed: /nope/w: no such file\nsed ended with 4\n4\n"),
    ) && same(
        &shell_on(&alloc::format!(
            "printf '{}\\n' | sed 's/\\(\\(a*\\)*\\)*\\1x/y/'; echo $?",
            "a".repeat(40)
        ))?,
        &ran(
            0,
            b"sed: the back-references leave too many ways to match this text\n\
              sed ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "sed should end with 4 when it cannot make a file to write, and stop a search too long",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'p\\n\\nk\\n' > /System/bad.sed || exit 9\n\
         sed -n -f /System/bad.sed; rm /System/bad.sed",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/bad.sed: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"sed: 3:1: k is no command\nsed ended with 1\n",
        "sed should say on which line of its script file it is wrong",
    )
}

/// A line longer than a message comes into sed whole, however the pipe
/// cuts it, and goes out whole: matched, counted, changed throughout and
/// printed.
fn sed_on_wide_lines() -> Outcome {
    let long = "x".repeat(300);
    let checked = same(
        &shell_on(&alloc::format!(
            "echo {long}b | sed 's/x*b/y/'\n\
             echo {long} | sed -n '$='\n\
             echo {long} | sed 's/x/z/g' | wc\n\
             echo a{long} | sed -n '/^ax*$/p' | wc"
        ))?,
        &ran(0, b"y\n1\n1 1 301\n1 1 302\n"),
    );
    expect(
        checked,
        "sed should take a line longer than a message whole",
    )
}

/// sed answers as the other tools do in a pipeline: lines 2 and 3 as head
/// and tail take them, every line marked as grep counts them, and sorted
/// lines taken once as uniq leaves them.
fn sed_among_the_tools() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "sed -n '2,3p' < {boot} | wc; head -n 3 {boot} | tail -n 2 | wc\n\
         sed 's/^/> /' {boot} | grep -c '^> '; sed -n '$=' {boot}\n\
         sort {boot} | uniq | wc; sort {boot} | sed '$!N;/^\\(.*\\)\\n\\1$/!P;D' | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    expect(
        ended == Ok(Ended::Exited(0))
            && lines.len() == 6
            && lines[0] == lines[1]
            && lines[2] == lines[3]
            && lines[4] == lines[5]
            && lines[3].parse::<usize>().is_ok_and(|count| count > 2),
        "sed should take the lines head, tail, grep and uniq take",
    )
}

/// expr works out arithmetic by precedence, matches at the start of a
/// text, ends with 1 for a value that is zero and says what it cannot
/// read or work out; sh scripts count with it.
fn expr_on_its_words() -> Outcome {
    let checked = same(
        &shell_on("expr 1 + 2 '*' 3; expr abc : 'a\\(.\\)'; expr 0; echo $?")?,
        &ran(0, b"7\nb\n0\nexpr ended with 1\n1\n"),
    ) && same(
        &shell_on("expr 1 / 0; expr 1 +; expr; echo $?")?,
        &ran(
            0,
            b"expr: a division by zero\nexpr ended with 3\n\
              expr: an expression ends too soon\nexpr ended with 2\n\
              expr: an expression ends too soon\nexpr ended with 2\n2\n",
        ),
    ) && same(
        &shell_on("#!/bin/sh\ni=0; while [ $i -lt 3 ]; do i=$(expr $i + 1); done; echo $i")?,
        &ran(0, b"3\n"),
    ) && same(
        &shell_on(
            "expr 'a b' : 'a \\(.\\)'; expr abc '<' abd; expr 10 '<' 9; expr 10 '<' 9a; echo $?",
        )?,
        &ran(0, b"b\n1\n0\nexpr ended with 1\n1\n0\n"),
    );
    expect(
        checked,
        "expr should work out its words, end as POSIX says, and count in a script",
    )
}

/// sleep waits as long as it is told, in ticks of the timer, and says
/// what is no time to wait.
fn sleep_waits() -> Outcome {
    let before = crate::ticks();
    let slept = shell_on("sleep 0.2; echo $?")?;
    let waited = crate::ticks() - before;
    expect(
        same(&slept, &ran(0, b"0\n")) && waited >= 20,
        "sleep 0.2 should wait twenty ticks at least",
    )?;
    expect(
        same(
            &shell_on("sleep x; sleep -- 0; echo $?; sleep 1 2")?,
            &ran(
                1,
                b"sleep: x is no number of seconds\nsleep ended with 1\n0\n\
                  sleep: takes one time\nsleep ended with 1\n",
            ),
        ),
        "sleep should take a -- and refuse what is no time, or two",
    )
}

/// cmp finds a file the same as itself, says which of two ended first,
/// with - for the standard input, keeps quiet with -s, says what it could
/// not read, and says where two files differ first or, with -l, each byte
/// that differs.
fn cmp_on_files() -> Outcome {
    let checked = same(
        &shell_on("cmp /a; cmp -x /a /b")?,
        &ran(
            2,
            b"cmp: compares two files\ncmp ended with 2\n\
              cmp: -x is no option\ncmp ended with 2\n",
        ),
    );
    expect(
        checked,
        "cmp should say what words it cannot take, and end with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let checked = same(
        &shell_on(&alloc::format!(
            "cmp {boot} {boot}; echo $?\n\
             head -n 1 {boot} | cmp - {boot}; echo $?\n\
             cmp -s {boot} /nope; echo $?"
        ))?,
        &ran(
            0,
            b"0\ncmp: EOF on -\ncmp ended with 1\n1\ncmp ended with 2\n2\n",
        ),
    );
    expect(
        checked,
        "cmp should find a file the same as itself and say which of two ended first",
    )?;
    let checked = same(
        &shell_on(&alloc::format!(
            "cmp /System/Boot {boot}; cmp -s /System/Boot {boot}; echo $?"
        ))?,
        &ran(
            0,
            b"cmp: /System/Boot: a directory, not a file\ncmp ended with 2\n\
              cmp ended with 2\n2\n",
        ),
    );
    expect(
        checked,
        "cmp should say what it could not read, but not with -s, and end with 2",
    )?;
    let (ended, said) = shell_on(
        "printf 'abc\\n' > /System/c1 || exit 9\n\
         printf 'abX\\n' > /System/c2\n\
         cmp /System/c1 /System/c2; cmp -l /System/c1 /System/c2\n\
         rm /System/c1 /System/c2",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/c1: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"/System/c1 /System/c2 differ: char 3, line 1\ncmp ended with 1\n\
                     3 143 130\ncmp ended with 1\n",
        "cmp should say where two files differ first, and with -l each byte",
    )
}

/// comm says what words it cannot take, finds every line of a file in
/// both when it compares the file with itself, and puts the lines of two
/// sorted files in their three columns.
fn comm_on_files() -> Outcome {
    let checked = same(
        &shell_on("comm /a; comm -4 /a /b")?,
        &ran(
            2,
            b"comm: compares two files\ncomm ended with 2\n\
              comm: -4 is no option\ncomm ended with 2\n",
        ),
    );
    expect(
        checked,
        "comm should say what words it cannot take, and end with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "comm -12 {boot} {boot} | wc; wc < {boot}; comm -3 {boot} - < {boot} | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let counts: Vec<&str> = text.lines().collect();
    expect(
        ended == Ok(Ended::Exited(0))
            && counts.len() == 3
            && counts[0] == counts[1]
            && counts[2] == "0 0 0",
        "comm should find every line of a file in both when it compares the file with itself",
    )?;
    let (ended, said) = shell_on(
        "printf 'a\\nb\\nd\\n' > /System/c1 || exit 9\n\
         printf 'b\\nc\\nd\\n' > /System/c2\n\
         comm /System/c1 /System/c2; rm /System/c1 /System/c2",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/c1: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"a\n\t\tb\n\tc\n\t\td\n",
        "comm should put each line in the column of where it is",
    )
}

/// paste puts lines side by side, each - taking the next line of the
/// standard input, joins a file's lines into one with -s and its own
/// delimiters with -d, and gives a file back as it was when the line
/// feed joins it again.
fn paste_joins_lines() -> Outcome {
    let checked = same(
        &shell_on(
            "paste; printf 'a\\nb\\nc\\n' | paste - -; printf '1\\n2\\n3\\n' | paste -s -d , -\n\
             paste /nope; echo $?",
        )?,
        &ran(
            0,
            b"paste: needs a file to join\npaste ended with 1\na\tb\nc\t\n1,2,3\n\
              paste: /nope: no such file\npaste ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "paste should join lines side by side, and a file's lines into one with -s",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "paste -s -d '\\n' {boot} | wc; wc < {boot}; paste {boot} {boot} | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let counts: Vec<Vec<u64>> = text
        .lines()
        .map(|line| line.split(' ').filter_map(|n| n.parse().ok()).collect())
        .collect();
    expect(
        ended == Ok(Ended::Exited(0))
            && counts.len() == 3
            && counts[0] == counts[1]
            && counts[2].first() == counts[1].first()
            && counts[2].get(1).copied() == counts[1].get(1).map(|words| words * 2),
        "paste should give a file back joined by its line feeds, and two side by side",
    )
}

/// fold breaks lines at the width, or after a blank with -s, refuses a
/// width of none, and folding a file only adds the line feeds it breaks
/// at.
fn fold_breaks_lines() -> Outcome {
    let checked = same(
        &shell_on(
            "echo abcdefghij | fold -w 4; echo 'the quick brown fox' | fold -s -w 10; fold -w 0",
        )?,
        &ran(
            1,
            b"abcd\nefgh\nij\nthe quick \nbrown fox\nfold: 0 is no width\nfold ended with 1\n",
        ),
    );
    expect(
        checked,
        "fold should break lines at the width, or after a blank with -s",
    )?;
    let checked = same(
        &shell_on("fold /nope; echo $?; printf 'ab\\tc\\n' | fold -b -w 3 -")?,
        &ran(
            0,
            b"fold: /nope: no such file\nfold ended with 1\n1\nab\t\nc\n",
        ),
    );
    expect(
        checked,
        "fold should say which file it cannot read, and take - for its input",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "wc < {boot}; fold -w 1000 {boot} | wc; fold -b -w 5 {boot} | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let counts: Vec<Vec<u64>> = text
        .lines()
        .map(|line| line.split(' ').filter_map(|n| n.parse().ok()).collect())
        .collect();
    let added = match (counts.first(), counts.get(2)) {
        (Some(whole), Some(folded)) if whole.len() == 3 && folded.len() == 3 => {
            folded[0] > whole[0] && folded[2] - whole[2] == folded[0] - whole[0]
        }
        _ => false,
    };
    expect(
        ended == Ok(Ended::Exited(0)) && counts.len() == 3 && counts[0] == counts[1] && added,
        "fold should leave lines narrower than the width, and add only line feeds",
    )
}

/// nl numbers the lines that are not empty, as wide and in the format it
/// is told, refuses an option it does not know, and with -b a numbers
/// every line of a file.
fn nl_numbers_lines() -> Outcome {
    let checked = same(
        &shell_on("printf 'a\\n\\nb\\n' | nl; printf 'a\\nb\\n' | nl -n rz -w 3 -s :; nl -x")?,
        &ran(
            1,
            b"     1\ta\n      \t\n     2\tb\n001:a\n002:b\nnl: -x is no option\nnl ended with 1\n",
        ),
    );
    expect(checked, "nl should number lines as it is told")?;
    let checked = same(
        &shell_on(
            "nl /a /b; printf '\\\\:\\\\:\\na\\n\\\\:\\\\:\\\\:\\n\\\\:\\\\:\\nb\\n' | nl -w 1\n\
             printf '\\\\:\\\\:\\\\:\\n\\\\:\\\\:\\na\\n\\\\:\\\\:\\\\:\\n\\\\:\\\\:\\nb\\n' | nl -p -w 1",
        )?,
        &ran(
            0,
            b"nl: numbers one file\nnl ended with 1\n\n1\ta\n\n\n1\tb\n\n\n1\ta\n\n\n2\tb\n",
        ),
    );
    expect(
        checked,
        "nl should number one file, and each logical page from 1 unless -p",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "wc < {boot}; nl -b a {boot} | wc; nl -b a {boot} | tail -n 1; nl -b n {boot} | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    let count = |line: &str| line.split(' ').next().and_then(|n| n.parse::<u64>().ok());
    let last = lines
        .get(2)
        .and_then(|line| line.trim_start().split('\t').next()?.parse::<u64>().ok());
    let numbers = |line: Option<&&str>| -> Vec<u64> {
        line.map_or(Vec::new(), |line| {
            line.split(' ').filter_map(|n| n.parse().ok()).collect()
        })
    };
    let (whole, none) = (numbers(lines.first()), numbers(lines.get(3)));
    let padded = whole.len() == 3
        && none.len() == 3
        && none[0] == whole[0]
        && none[2] == whole[2] + 7 * whole[0];
    expect(
        ended == Ok(Ended::Exited(0))
            && lines.len() == 4
            && lines.first().and_then(|l| count(l)) == lines.get(1).and_then(|l| count(l))
            && last == lines.first().and_then(|l| count(l))
            && padded,
        "nl -b a should number every line of a file, and -b n set each seven bytes in",
    )
}

/// cksum writes POSIX's checksum and the bytes of its input, and says
/// what it could not read.
fn cksum_sums() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'hello\\n' | cksum; printf '' | cksum; printf '%0200d' 0 | cksum; cksum /nope; echo $?",
        )?,
        &ran(
            0,
            b"3015617425 6\n4294967295 0\n2535012215 200\ncksum: /nope: no such file\ncksum ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "cksum should write POSIX's checksum and the bytes, every one of them",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    // A file sums the same named as through the standard input.
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "cksum {boot}; cksum < {boot}; wc -c < {boot}"
    ))?;
    let text = String::from_utf8_lossy(&said);
    let lines: Vec<&str> = text.lines().collect();
    // Its count of bytes is all the file's, which takes many messages.
    let counted = lines.get(1).and_then(|line| line.split(' ').nth(1));
    expect(
        ended == Ok(Ended::Exited(0))
            && lines.len() == 3
            && lines[0] == alloc::format!("{} {boot}", lines[1])
            && counted == Some(lines[2].trim()),
        "cksum should sum a file named as it sums it through the standard input, every byte of it",
    )
}

/// diff says what words it cannot take, finds a file the same as itself,
/// writes an ed script that cuts what its standard input lacks, says what
/// it could not find, and says how two files and two directories differ.
fn diff_on_files() -> Outcome {
    let checked = same(
        &shell_on("diff /a; diff -x /a /b; diff -U")?,
        &ran(
            2,
            b"diff: compares two files\ndiff ended with 2\n\
              diff: -x is no option\ndiff ended with 2\n\
              diff: -U wants a number of lines\ndiff ended with 2\n",
        ),
    );
    expect(
        checked,
        "diff should say what words it cannot take, and end with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "diff {boot} {boot}; echo $?\n\
         head -n 2 {boot} | diff -e {boot} -\n\
         diff /nope {boot}; wc -l < {boot}"
    ))?;
    // The script cuts from the third line to the last, which wc counts.
    let cut = said.strip_prefix(b"0\n3,".as_slice()).and_then(|rest| {
        let (last, lines) = rest.split_at(rest.iter().position(|&b| b == b'd')?);
        let lines = lines.strip_prefix(
            b"d\ndiff ended with 1\ndiff: /nope: no such file\ndiff ended with 2\n".as_slice(),
        )?;
        Some((last, lines.trim_ascii()))
    });
    expect(
        ended == Ok(Ended::Exited(0))
            && cut.is_some_and(|(last, lines)| {
                !last.is_empty() && last.iter().all(u8::is_ascii_digit) && last == lines
            }),
        "diff should find a file the same as itself, cut what its input lacks to the last line and say what it could not find",
    )?;
    let (ended, said) = shell_on(
        "md /System/E || exit 9\n\
         md /System/F\n\
         printf 'x\\ny\\nz\\n' > /System/E/f\n\
         printf 'x\\nY\\nz\\n' > /System/F/f\n\
         echo k > /System/E/g\n\
         diff /System/E/f /System/F/f; diff -u /System/E /System/F\n\
         printf '%0200d\\nq\\n' 0 > /System/E/h; printf '%0200d\\nr\\n' 0 | diff /System/E/h -\n\
         rm /System/E/f /System/E/g /System/E/h /System/F/f; rd /System/E; rd /System/F",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/E: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"2c2\n< y\n---\n> Y\ndiff ended with 1\n\
                     diff -u /System/E/f /System/F/f\n--- /System/E/f\n+++ /System/F/f\n\
                     @@ -1,3 +1,3 @@\n x\n-y\n+Y\n z\nOnly in /System/E: g\ndiff ended with 1\n\
                     2c2\n< q\n---\n> r\ndiff ended with 1\n",
        "diff should say how two files differ, and two directories in the unified form, \
         each file read to its end",
    )
}

/// patch says what words it cannot take, puts a diff into the file its
/// header names in the directory -d gives, takes it out again with -R
/// keeping the file as it was with -b, makes a file a patch makes, and
/// keeps a hunk that finds no place in a file of rejects. The files take
/// more than one message to read.
fn patch_puts_diffs_in() -> Outcome {
    let checked = same(
        &shell_on("patch -x; patch -p x; patch a b")?,
        &ran(
            2,
            b"patch: -x is no option\npatch ended with 2\n\
              patch: -p wants a number, not x\npatch ended with 2\n\
              patch: patches one file\npatch ended with 2\n",
        ),
    );
    expect(
        checked,
        "patch should say what words it cannot take, and end with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/Q || exit 9\n\
         printf 'l%s\\n' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 > /System/Q/f\n\
         printf 'l%s\\n' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 B 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 > /System/Q/g\n\
         cp /System/Q/f /System/Q/o; cd /System/Q; diff -u f g > p.diff; cd /\n\
         patch -d /System/Q < /System/Q/p.diff; cmp /System/Q/f /System/Q/g && echo same\n\
         patch -R -b -i /System/Q/p.diff /System/Q/f\n\
         cmp /System/Q/f /System/Q/o && cmp /System/Q/f.orig /System/Q/g && echo back",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/Q: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"diff ended with 1\nsame\nback\n",
        "patch should put a diff into the file it names and take it out again with -R, keeping the file with -b",
    )?;
    let checked = same(
        &shell_on(
            "cd /System/Q; printf '' > e; printf 'n\\n' > n; diff -u e n > c.diff; rm e n\n\
             patch < c.diff; cat n",
        )?,
        &ran(0, b"diff ended with 1\nn\n"),
    );
    expect(
        checked,
        "patch should make the file a patch makes when neither of its names is there",
    )?;
    let checked = same(
        &shell_on(
            "printf 'x\\ny\\n' > /System/Q/h\n\
             patch /System/Q/h < /System/Q/p.diff; echo $?\n\
             cmp /System/Q/h.rej /System/Q/p.diff && echo kept\n\
             cd /System/Q; rm f g o p.diff f.orig c.diff n h h.rej; cd /; rd /System/Q",
        )?,
        &ran(
            0,
            b"patch: /System/Q/h: hunk 1 found no place\n\
              patch: /System/Q/h: 1 of 1 hunks found no place, kept in /System/Q/h.rej\n\
              patch ended with 1\n1\nkept\n",
        ),
    );
    expect(
        checked,
        "patch should keep a hunk that finds no place in a file of rejects, and end with 1",
    )
}

/// find says what words it cannot take, walks the programs' directory
/// testing names and types, runs a program on what it found with -exec,
/// keeps out of a directory with -prune, and walks a tree it made with
/// -depth.
fn find_walks_trees() -> Outcome {
    let checked = same(
        &shell_on("find; find a -foo; find a -mtime 1")?,
        &ran(
            1,
            b"find: finds from a path at least\nfind ended with 1\n\
              find: -foo is no primary\nfind ended with 1\n\
              find: -mtime: Meridian's files keep no times\nfind ended with 1\n",
        ),
    );
    expect(
        checked,
        "find should say what words it cannot take, and end with 1",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let programs = "/System/Boot/meridian/programs";
    let checked = same(
        &shell_on(&alloc::format!(
            "find {programs} -name 'he*' -type f\n\
             find {programs} -name 'wc*' -exec echo '{{}}' +\n\
             find /System/Boot/meridian -prune; find /nope"
        ))?,
        &ran(
            1,
            alloc::format!(
                "{programs}/head.mexec\n{programs}/head_again.mexec\n\
                 {programs}/hello.mexec\n{programs}/hello_again.mexec\n\
                 {programs}/wc.mexec {programs}/wc_again.mexec\n\
                 /System/Boot/meridian\nfind: /nope: no such file\nfind ended with 1\n"
            )
            .as_bytes(),
        ),
    );
    expect(
        checked,
        "find should test names and types, run a program on what it found, and keep out of a pruned directory",
    )?;
    // A directory is no program to start, which find says, ending with 1.
    let (ended, said) = shell_on(
        "find /System/Boot/meridian -prune -exec /System/Boot/meridian '{}' ';'; echo $?",
    )?;
    let said = String::from_utf8_lossy(&said);
    expect(
        ended == Ok(Ended::Exited(0))
            && said.starts_with("find: /System/Boot/meridian: ")
            && said.ends_with("\nfind ended with 1\n1\n")
            && said.lines().count() == 3,
        "find should say a program it could not start, and end with 1",
    )?;
    let (ended, said) = shell_on(
        "md /System/W || exit 9\n\
         md /System/W/s; echo x > /System/W/a; echo y > /System/W/s/b\n\
         find /System/W -depth; find /System/W -type f -exec wc -c '{}' ';'\n\
         rm /System/W/a /System/W/s/b; rd /System/W/s; rd /System/W",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/W: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"/System/W/a\n/System/W/s/b\n/System/W/s\n/System/W\n2 /System/W/a\n2 /System/W/s/b\n",
        "find should walk a tree it is given, its files first with -depth",
    )
}

/// csplit says what words it cannot take, cuts a file of more than one
/// message at a line, before a match and past a skipped part, saying the
/// size of each file, and removes what it made when a pattern finds no
/// line.
fn csplit_cuts_files() -> Outcome {
    let checked = same(
        &shell_on("csplit; csplit f; csplit -x f 1")?,
        &ran(
            1,
            b"csplit: cuts a file at the lines it is told\ncsplit ended with 1\n\
              csplit: cuts a file at the lines it is told\ncsplit ended with 1\n\
              csplit: -x is no option\ncsplit ended with 1\n",
        ),
    );
    expect(
        checked,
        "csplit should say what words it cannot take, and end with 1",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/C || exit 9\n\
         cd /System/C; printf 'l%s\\n' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 > in\n\
         csplit in 10 /l25/ %l35%; head -n 1 xx02",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/C: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"27\n60\n24\nl35\n",
        "csplit should cut a file at a line, before a match and past a skipped part",
    )?;
    let checked = same(
        &shell_on(
            "cd /System/C; csplit -f e in /nope/; echo $?; test -f e00; echo $?\n\
             rm in xx00 xx01 xx02; cd /; rd /System/C",
        )?,
        &ran(
            0,
            b"151\ncsplit: /nope/: no line matches it\ncsplit ended with 1\n1\n1\n",
        ),
    );
    expect(
        checked,
        "csplit should remove what it made when a pattern finds no line",
    )
}

/// pathchk says what words it cannot take, which pathnames are not
/// portable or no names a FAT volume takes, finds the loader's own path
/// fine and one through nothing there too, and with -P refuses a name
/// that begins with a hyphen.
fn pathchk_checks_names() -> Outcome {
    let checked = same(
        &shell_on(
            "pathchk -p 'a b' ok; echo $?\n\
             pathchk 'x:y' /System/Boot/meridian/boot.meri /nope/a/b; echo $?",
        )?,
        &ran(
            0,
            b"pathchk: a b: the name a b holds ' ', which is no portable character\n\
              pathchk ended with 1\n1\n\
              pathchk: x:y: the name x:y is none a FAT volume takes\n\
              pathchk ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "pathchk should say which pathnames are not portable or no FAT names, and pass the rest",
    )?;
    let checked = same(
        &shell_on("pathchk -P -- -a; pathchk; pathchk /System/Boot; echo $?")?,
        &ran(
            0,
            b"pathchk: -a: the name -a begins with -\npathchk ended with 1\n\
              pathchk: checks a pathname at least\npathchk ended with 1\n0\n",
        ),
    );
    expect(
        checked,
        "pathchk should refuse a name beginning with - with -P, and want a pathname",
    )
}

/// dd says what operands it cannot take, turns what comes through a
/// pipe into capitals, copies blocks of a file past those it skips, and
/// writes into the middle of a file it keeps the rest of, saying the
/// blocks each time.
fn dd_copies_blocks() -> Outcome {
    let checked = same(
        &shell_on("dd x; dd bs=0; dd conv=xyz")?,
        &ran(
            1,
            b"dd: x is no operand\ndd ended with 1\n\
              dd: bs= wants a size, not 0\ndd ended with 1\n\
              dd: conv=xyz is no conversion\ndd ended with 1\n",
        ),
    );
    expect(
        checked,
        "dd should say what operands it cannot take, and end with 1",
    )?;
    let checked = same(
        &shell_on("printf 'Hello\\n' | dd conv=ucase; printf 'abcdef' | dd bs=2 skip=1")?,
        &ran(
            0,
            b"HELLO\n0+1 records in\n0+1 records out\ncdef2+0 records in\n2+0 records out\n",
        ),
    );
    expect(
        checked,
        "dd should turn what comes through a pipe into capitals, and read past the blocks it skips",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/D || exit 9\n\
         printf '0123456789abcdefghij' > /System/D/in\n\
         dd if=/System/D/in of=/System/D/out bs=5 skip=1 count=2; cat /System/D/out; echo",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/D: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    // A step at a time, so that a step that never ends is known by the
    // shells that did.
    let steps: [(&str, &[u8], &str); 6] = [
        (
            "",
            b"2+0 records in\n2+0 records out\n56789abcde\n",
            "dd should copy blocks past those it skips",
        ),
        (
            "printf 'XY' | dd of=/System/D/out bs=2 seek=1 conv=notrunc; cat /System/D/out; echo",
            b"1+0 records in\n1+0 records out\n56XY9abcde\n",
            "dd should write into a file it keeps the rest of",
        ),
        (
            "printf 'XY' | dd of=/System/D/out bs=2 seek=2",
            b"1+0 records in\n1+0 records out\n",
            "dd should write past the blocks it seeks past in a file made anew",
        ),
        (
            "cat /System/D/out; echo",
            b"56XYXY\n",
            "dd should keep what lies before the blocks it seeks past",
        ),
        (
            "printf '%0300d' 0 > /System/D/big; printf 'XY' | dd of=/System/D/big bs=1 seek=200\n\
             cksum < /System/D/big",
            b"2+0 records in\n2+0 records out\n2305766754 202\n",
            "dd should keep what lies before the blocks it seeks past, more of it than one read gives",
        ),
        (
            "rm /System/D/in /System/D/out /System/D/big; rd /System/D",
            b"",
            "what dd wrote should be removed",
        ),
    ];
    let mut done = (ended, said);
    for (at, (script, wanted, why)) in steps.into_iter().enumerate() {
        if at > 0 {
            done = shell_on(script)?;
        }
        expect(same(&done, &ran(0, wanted)), why)?;
    }
    Ok(())
}

/// rm as a program, the shell's own rm being a command of its own: it
/// says what words it cannot take, removes a file and says one not
/// there, keeps a directory without -r and removes it with it.
fn rm_removes_files() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs rm; echo | xargs rm -x")?,
        &ran(
            123,
            b"rm: removes a file at least\nxargs ended with 123\n\
              rm: -x is no option\nxargs ended with 123\n",
        ),
    );
    expect(checked, "rm should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/R || exit 9\n\
         md /System/R/d; echo x > /System/R/d/f; echo y > /System/R/g\n\
         echo /System/R/g /System/R/nope | xargs rm; echo $?\n\
         echo /System/R/d | xargs rm; echo /System/R/d | xargs rm -r; test -d /System/R/d; echo $?\n\
         rd /System/R",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/R: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"rm: /System/R/nope: no such file\nxargs ended with 123\n123\n\
                     rm: /System/R/d: a directory, which goes only with -R\nxargs ended with 123\n1\n",
        "rm should remove a file, say one not there, and a directory only with -r",
    )
}

/// mkdir as a program says what words it cannot take, makes a directory
/// with those above it with -p, and says one that is there already.
fn mkdir_makes_directories() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs mkdir; echo | xargs mkdir -m 7 a")?,
        &ran(
            123,
            b"mkdir: makes a directory at least\nxargs ended with 123\n\
              mkdir: -m: Meridian's files have no modes\nxargs ended with 123\n",
        ),
    );
    expect(checked, "mkdir should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/M || exit 9\n\
         echo /System/M/a/b | xargs mkdir -p; test -d /System/M/a/b && echo made\n\
         echo /System/M/a | xargs mkdir; rd /System/M/a/b; rd /System/M/a; rd /System/M",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/M: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"made\nmkdir: /System/M/a: there already\nxargs ended with 123\n",
        "mkdir should make the directories above with -p, and say one there already",
    )
}

/// rmdir as a program says what words it cannot take, goes up a path
/// with -p as long as each directory goes, and keeps a file.
fn rmdir_removes_directories() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs rmdir")?,
        &ran(
            123,
            b"rmdir: removes a directory at least\nxargs ended with 123\n",
        ),
    );
    expect(checked, "rmdir should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/E || exit 9\n\
         md /System/E/a; md /System/E/a/b; echo x > /System/E/f\n\
         echo /System/E/a/b | xargs rmdir -p; test -d /System/E/a; echo $?\n\
         echo /System/E/f | xargs rmdir; md /System/E/g; echo /System/E/g | xargs rmdir\n\
         test -d /System/E/g; echo $?; rm /System/E/f; rd /System/E",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/E: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"rmdir: /System/E: holds something\nxargs ended with 123\n1\n\
                     rmdir: /System/E/f: no directory\nxargs ended with 123\n1\n",
        "rmdir should go up a path with -p as long as each goes, keep a file, and end with 0 once all went",
    )
}

/// awk splits records into fields, reads and writes commands, ends with
/// the status its program gives, and says where a program is wrong.
fn awk_runs_programs() -> Outcome {
    let checked = same(
        &shell_on(
            "echo 'a b' | awk '{ print $2, $1 }'; printf '%0200d\\n' 0 | awk '{ print length($0) }'\n\
             echo a:b | awk -F: -v x=5 '{ print $2, x * 2 }'",
        )?,
        &ran(0, b"b a\n200\nb 10\n"),
    );
    expect(
        checked,
        "awk should split records into fields, take -F and -v, and read a record past one message",
    )?;
    let checked = same(
        &shell_on(
            "awk 'BEGIN { print \"x\" | \"cat\"; close(\"cat\"); \"echo hi\" | getline v; print v; print system(\"exit 4\"); exit 3 }'",
        )?,
        &ran(3, b"x\nhi\n4\nawk ended with 3\n"),
    );
    expect(
        checked,
        "awk should write to a command, read from one, run one with system, and end with the status exit gives",
    )?;
    let checked = same(
        &shell_on(
            "awk 'BEGIN { print \"w\" | \"sort\"; print \"x\"; print fflush(); close(\"sort\") }'\n\
             awk 'BEGIN { print \"b\" | \"sleep 1; sort\" }'; echo after",
        )?,
        &ran(0, b"x\nw\n0\nb\nafter\n"),
    );
    expect(
        checked,
        "awk's fflush should write what it holds before a command writes, and awk wait for its commands as it ends",
    )?;
    let checked = same(
        &shell_on("awk 'BEGIN {'; awk -x")?,
        &ran(
            2,
            b"awk: line 1: } was wanted, not the end\nawk ended with 2\n\
              awk: -x is no option\nawk ended with 2\n",
        ),
    );
    expect(
        checked,
        "awk should say where a program is wrong and what words it cannot take",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/A || exit 9\n\
         printf '# This rule comes after a line long enough that awk reads the file that holds its program in more than one piece, as it has to.\\n\
         NR == 2 { print $1 > \"/System/A/o\" }\\n' > /System/A/p\n\
         printf 'x 1\\ny 2\\n' | awk -f /System/A/p\n\
         awk '{ print \"got\", $0 }' /System/A/o\n\
         rm /System/A/p /System/A/o; rd /System/A",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/A: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        same(&(ended, said), &ran(0, b"got y\n")),
        "awk should read its program from a file of more than one piece, and write a file that it reads back",
    )
}

/// ls as a program refuses what Meridian's files cannot answer, and
/// lists a directory and a file as its options ask.
fn ls_lists_directories() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs ls -l; echo /nope | xargs ls")?,
        &ran(
            123,
            b"ls: -l: Meridian's files have no owners, modes, times, links or numbers yet\n\
              xargs ended with 123\nls: /nope: no such file\nxargs ended with 123\n",
        ),
    );
    expect(
        checked,
        "ls should refuse what Meridian's files cannot answer, and say a path not there",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/L || exit 9\n\
         md /System/L/d\n\
         printf 'ab' > /System/L/b\n\
         printf '' > /System/L/a\n\
         echo /System/L | xargs ls -p\n\
         echo /System/L/b /System/L/d | xargs ls -s\n\
         rm /System/L/a /System/L/b; rd /System/L/d; rd /System/L",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/L: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(0, b"a\nb\nd/\n1 /System/L/b\n\n/System/L/d:\ntotal 0\n"),
        ),
        "ls should list a directory with a / after the one within, and a file and a directory with their blocks",
    )
}

/// cp as a program says what words it cannot take, copies a directory
/// with all it holds, and copies no file onto itself.
fn cp_copies_files() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs cp -x")?,
        &ran(123, b"cp: -x is no option\nxargs ended with 123\n"),
    );
    expect(checked, "cp should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/C || exit 9\n\
         md /System/C/t\n\
         printf 'ab' > /System/C/t/f\n\
         echo /System/C/t /System/C/u | xargs cp -R\n\
         cat /System/C/u/f; echo\n\
         echo /System/C/t/f /System/C/t/f | xargs cp\n\
         rm /System/C/t/f /System/C/u/f; rd /System/C/t; rd /System/C/u; rd /System/C",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/C: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(
                0,
                b"ab\ncp: /System/C/t/f and /System/C/t/f are one file\nxargs ended with 123\n",
            ),
        ),
        "cp should copy a directory with what it holds, and refuse a file onto itself",
    )
}

/// mv as a program says what words it cannot take, and moves a file to
/// another name, into a directory, and a directory with what it holds.
fn mv_moves_files() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs mv")?,
        &ran(
            123,
            b"mv: moves a file to another name, or files into a directory\nxargs ended with 123\n",
        ),
    );
    expect(checked, "mv should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/M || exit 9\n\
         md /System/M/d\n\
         printf 'x' > /System/M/a\n\
         echo /System/M/a /System/M/b | xargs mv\n\
         echo /System/M/b /System/M/d | xargs mv\n\
         echo /System/M/d /System/M/e | xargs mv\n\
         cat /System/M/e/b; echo\n\
         rm /System/M/e/b; rd /System/M/e; rd /System/M",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/M: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        same(&(ended, said), &ran(0, b"x\n")),
        "mv should move a file to another name, into a directory, and the directory with it",
    )?;
    // From the boot volume, which is only read, everything is copied and
    // then nothing can go.
    let from = "/System/Boot/meridian/licences";
    let checked = same(
        &shell_on(&alloc::format!(
            "echo {from} /System/L | xargs mv\n\
             cmp {from}/inter.txt /System/L/inter.txt && cmp {from}/jetbrains_mono.txt /System/L/jetbrains_mono.txt && cmp {from}/shim.txt /System/L/shim.txt && echo copied\n\
             rm /System/L/inter.txt /System/L/jetbrains_mono.txt /System/L/shim.txt; rd /System/L"
        ))?,
        &ran(
            0,
            alloc::format!(
                "mv: {from} to /System/L: on a volume that is only read\nxargs ended with 123\ncopied\n"
            )
            .as_bytes(),
        ),
    );
    expect(
        checked,
        "mv should copy a directory to another volume, every file whole, and say why it cannot remove it after",
    )
}

/// strings finds the runs of printable characters among bytes that are
/// none, with their places, and says what words it cannot take.
fn strings_finds_text() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'ab\\001hello\\000xyzw\\n' | strings; printf 'xx\\000abcdef' | strings -t d -n 5; strings -x",
        )?,
        &ran(
            1,
            b"hello\nxyzw\n3 abcdef\nstrings: -x is no option\nstrings ended with 1\n",
        ),
    );
    expect(
        checked,
        "strings should find the runs of printable characters and their places",
    )?;
    let checked = same(
        &shell_on(
            "printf '%0200d\\n' 0 | strings | cksum; printf 'abcd\\n' | strings -; strings /nope",
        )?,
        &ran(
            1,
            b"2413893277 201\nabcd\nstrings: /nope: no such file\nstrings ended with 1\n",
        ),
    );
    expect(
        checked,
        "strings should read a run past one message, take - for its standard input, and say a file not there",
    )
}

/// du refuses what it cannot tell, and counts the blocks of a tree,
/// each directory after what it holds.
fn du_counts_blocks() -> Outcome {
    let checked = same(
        &shell_on("du -x; du /nope")?,
        &ran(
            1,
            b"du: -x: volumes cannot be told apart yet\ndu ended with 1\n\
              du: /nope: no such file\ndu ended with 1\n",
        ),
    );
    expect(
        checked,
        "du should refuse to tell volumes apart, and say a path not there",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/U || exit 9\n\
         md /System/U/d\n\
         printf '%0600d' 0 > /System/U/d/f\n\
         printf 'x' > /System/U/g\n\
         du -a /System/U; du -s /System/U\n\
         rm /System/U/d/f /System/U/g; rd /System/U/d; rd /System/U",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/U: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(
                0,
                b"2 /System/U/d/f\n2 /System/U/d\n1 /System/U/g\n3 /System/U\n3 /System/U\n",
            ),
        ),
        "du should count the blocks of a tree, each directory after what it holds",
    )
}

/// touch refuses times, makes a file not there, and with -c none.
fn touch_makes_files() -> Outcome {
    let checked = same(
        &shell_on("touch -t 1 f")?,
        &ran(
            1,
            b"touch: -t: Meridian's files have no times yet\ntouch ended with 1\n",
        ),
    );
    expect(
        checked,
        "touch should refuse the times Meridian's files do not have",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "touch /System/tf || exit 9\n\
         cat /System/tf | wc; touch -c /System/tg; cat < /System/tg\n\
         touch /System/Boot/meridian/boot.meri; echo $?; rm /System/tf",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"touch: /System/tf: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(0, b"0 0 0\n/System/tg: no such file\n0\n"),
        ),
        "touch should make an empty file, with -c none, and leave a file there on a volume only read",
    )
}

/// unlink removes the one file it names, and no directory.
fn unlink_removes_a_file() -> Outcome {
    let checked = same(
        &shell_on("unlink a b")?,
        &ran(1, b"unlink: removes one file\nunlink ended with 1\n"),
    );
    expect(checked, "unlink should take one file")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'x' > /System/uf || exit 9\n\
         unlink /System/uf; unlink /System/uf; unlink /System",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/uf: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(
                1,
                b"unlink: /System/uf: no such file\nunlink ended with 1\n\
                  unlink: /System: a directory, which unlink does not remove\nunlink ended with 1\n",
            ),
        ),
        "unlink should remove a file, then find none, and leave a directory",
    )
}

/// ed takes its commands from its standard input, reads and writes past
/// a message through commands, prompts, says what words it cannot take,
/// and edits a file with the script diff writes.
fn ed_edits_files() -> Outcome {
    let mut wanted = b"200\n".to_vec();
    wanted.extend_from_slice("0".repeat(200).as_bytes());
    wanted.extend_from_slice(b"\n201\n");
    let checked = same(
        // ed's % is the file's name, \% the character.
        &shell_on("printf 'r !printf \\\\%%0200d 0\\nw !cat\\nQ\\n' | ed")?,
        &ran(0, &wanted),
    );
    expect(
        checked,
        "ed should read what a command writes and give a command its lines, past one message each",
    )?;
    let checked = same(
        &shell_on("printf 'a\\nhello\\n.\\n,s/l*o/p/p\\n!echo y\\nQ\\n' | ed -p '> '")?,
        &ran(0, b"> > hep\n> y\n!\n> "),
    );
    expect(
        checked,
        "ed should prompt before each command, take text, substitute and run a command",
    )?;
    let checked = same(
        &shell_on("printf '1p\\n' | ed; ed -x; ed a b")?,
        &ran(
            2,
            b"?\ned ended with 1\n\
              ed: -x is no option\ned ended with 2\n\
              ed: edits one file\ned ended with 2\n",
        ),
    );
    expect(
        checked,
        "ed should end with 1 after a mistake, and with 2 for words it cannot take",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'a\\nb\\nc\\n' > /System/ea || exit 9\n\
         printf 'a\\nB\\nc\\nd\\n' > /System/eb\n\
         diff -e /System/ea /System/eb > /System/es; echo w >> /System/es\n\
         ed -s /System/ea < /System/es; cat /System/ea\n\
         printf ',s/[ac]/X/\\nw\\nq\\n' | ed /System/ea; cat /System/ea\n\
         rm /System/ea /System/eb /System/es",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/ea: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(0, b"diff ended with 1\na\nB\nc\nd\n8\n8\nX\nB\nX\nd\n"),
        ),
        "ed should edit a file with diff's script, and write it back changed",
    )
}

/// uuencode writes its input as text in both encodings, past a message,
/// and nothing for a file not there.
fn uuencode_writes_text() -> Outcome {
    let checked = same(
        &shell_on("echo hello | uuencode x; echo hello | uuencode -m - y")?,
        &ran(
            0,
            b"begin 644 x\n&:&5L;&\\*\n`\nend\nbegin-base64 644 y\naGVsbG8K\n====\n",
        ),
    );
    expect(
        checked,
        "uuencode should write its input in the historical encoding and in base64",
    )?;
    let checked = same(
        &shell_on("printf '%0200d' 0 | uuencode -m x | tail -n 2; uuencode /nope x; uuencode")?,
        &ran(
            1,
            b"MDAwMDAwMDAwMDAwMDAwMDAwMDA=\n====\n\
              uuencode: /nope: no such file\nuuencode ended with 1\n\
              uuencode: wants the name to decode to, after the file if there is one\nuuencode ended with 1\n",
        ),
    );
    expect(
        checked,
        "uuencode should read past one message, write nothing for a file not there, and say what it wants",
    )
}

/// uudecode makes again what uuencode wrote, in both encodings and past
/// a message, onto the standard output or into files, and says what is
/// not uuencode's.
fn uudecode_makes_files() -> Outcome {
    let checked = same(
        &shell_on(
            "echo hello | uuencode x | uudecode -o /dev/stdout\n\
             printf '%0200d' 0 | uuencode -m y | uudecode -o /dev/stdout | uuencode -m z | tail -n 2\n\
             echo hi | uudecode; uudecode -x",
        )?,
        &ran(
            1,
            b"hello\nMDAwMDAwMDAwMDAwMDAwMDAwMDA=\n====\n\
              uudecode: -: no line begins what uuencode writes\nuudecode ended with 1\n\
              uudecode: -x is no option\nuudecode ended with 1\n",
        ),
    );
    expect(
        checked,
        "uudecode should make again what uuencode wrote, past a message, and say what is not its",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'a\\nb\\n' > /System/ua || exit 9\n\
         uuencode /System/ua /System/ub | uudecode; cat /System/ub\n\
         uuencode -m /System/ua x | uudecode -o /System/uc; cat /System/uc\n\
         rm /System/ua /System/ub /System/uc",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/ua: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        same(&(ended, said), &ran(0, b"a\nb\na\nb\n")),
        "uudecode should make the file the text names, or the one -o names",
    )
}

/// bc works out numbers to any length and scale, with -l's functions,
/// reads a line past a message, breaks long lines, goes on past mistakes
/// in a program, and runs a program from a file.
fn bc_works_out_numbers() -> Outcome {
    let checked = same(
        &shell_on(
            "echo '2^500; scale=20; 1/3' | bc; echo 's(1); e(1)' | bc -l; printf '%0200d\\n' 0 | bc",
        )?,
        &ran(
            0,
            b"32733906078961418700131896968275991522166420460430647894832913680961\\\n\
              33796404674554883270092325904157150886684127560071009217256545885393\\\n\
              053328527589376\n.33333333333333333333\n\
              .84147098480789650665\n2.71828182845904523536\n0\n",
        ),
    );
    expect(
        checked,
        "bc should work out numbers past a line long, -l's functions, and read a line past a message",
    )?;
    let checked = same(
        &shell_on("printf '1/0; 2\\n3\\n' | bc; bc -x; bc /nope")?,
        &ran(
            1,
            b"bc: a division by zero\n3\n\
              bc: -x is no option\nbc ended with 1\n\
              bc: /nope: no such file\nbc ended with 1\n",
        ),
    );
    expect(
        checked,
        "bc should go on past a mistake in a program, and end with 1 for words and files it cannot take",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        // The file ends with quit, as bc would read its standard input
        // next.
        "printf 'define f(n) { if (n < 2) return (1); return (n * f(n - 1)) }\\nf(20)\\nquit\\n' > /System/bc.f || exit 9\n\
         bc /System/bc.f; rm /System/bc.f",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/bc.f: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        same(&(ended, said), &ran(0, b"2432902008176640000\n")),
        "bc should run a program from a file",
    )
}

/// compress, uncompress and zcat take what they are given through
/// pipes, past a message, and files on a disk to their names with .Z and
/// back, keep a file compress cannot make smaller, and say what is not
/// compress's.
fn compress_squeezes_files() -> Outcome {
    let checked = same(
        &shell_on(
            "echo hello | compress | uncompress; printf '%0300d\\n' 0 | compress -b 9 | zcat | cksum\n\
             echo hi | zcat; compress -x; uncompress /nope",
        )?,
        &ran(
            1,
            b"hello\n4020527664 301\n\
              zcat: -: not what compress writes\nzcat ended with 1\n\
              compress: -x is no option\ncompress ended with 1\n\
              uncompress: /nope.Z: no such file\nuncompress ended with 1\n",
        ),
    );
    expect(
        checked,
        "compress and its others should take what they are given through pipes, and say what they cannot",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf '%0300d\\n' 0 > /System/cz || exit 9\n\
         compress -v /System/cz; zcat /System/cz | cksum; uncompress /System/cz.Z; cksum < /System/cz\n\
         echo ab > /System/cs; compress /System/cs; cat /System/cs; rm /System/cz /System/cs",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/cz: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        same(
            &(ended, said),
            &ran(
                0,
                b"/System/cz: 89.36% smaller, now /System/cz.Z\n4020527664 301\n4020527664 301\n\
                  compress ended with 2\nab\n",
            ),
        ),
        "compress should put a file into its name with .Z and back, and keep one it cannot make smaller",
    )
}

/// file says what words it cannot take, and names text, empty files,
/// compressed data, directories, the UEFI loader and Meridian's ELF
/// programs on a disk.
fn file_names_kinds() -> Outcome {
    let checked = same(
        &shell_on("file; file -x; file /nope")?,
        &ran(
            1,
            b"file: names a file at least\nfile ended with 1\n\
              file: -x is no option\nfile ended with 1\n\
              file: /nope: no such file\nfile ended with 1\n",
        ),
    );
    expect(checked, "file should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'hello\\n' > /System/ft || exit 9\n\
         printf '' > /System/fe; echo abc | compress > /System/fz\n\
         file /System/ft /System/fe /System/fz /System/Boot /System/Boot/EFI/BOOT/BOOTX64.EFI\n\
         file /System/Boot/meridian/programs/file.mexec; rm /System/ft /System/fe /System/fz",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/ft: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    let wanted: &[u8] = b"/System/ft: ASCII text\n/System/fe: empty\n\
        /System/fz: data compressed by compress, codes of up to 16 bits\n\
        /System/Boot: directory\n\
        /System/Boot/EFI/BOOT/BOOTX64.EFI: PE32+ UEFI application, x86-64\n\
        /System/Boot/meridian/programs/file.mexec: ELF 64-bit ";
    expect(
        ended == Ok(Ended::Exited(0))
            && said.starts_with(wanted)
            && said.ends_with(b" for x86-64\n"),
        "file should name text, an empty file, compressed data, a directory, the loader and its own program",
    )
}

/// cat as a program says what words it cannot take and writes a file
/// twice when named twice, a piece at a time with -u.
fn cat_writes_files() -> Outcome {
    let checked = same(
        &shell_on("echo | xargs cat -x; echo /nope | xargs cat")?,
        &ran(
            123,
            b"cat: -x is no option\nxargs ended with 123\ncat: /nope: no such file\nxargs ended with 123\n",
        ),
    );
    expect(
        checked,
        "cat should say what words and files it cannot take, and end with 1",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "echo {boot} {boot} | xargs cat -u | wc -l; wc -l < {boot}"
    ))?;
    let counts: Vec<u64> = String::from_utf8_lossy(&said)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    expect(
        ended == Ok(Ended::Exited(0))
            && counts.len() == 2
            && counts[0] == 2 * counts[1]
            && counts[1] > 0,
        "cat should write a file named twice twice",
    )
}

/// tsort writes items in an order their pairs keep, and says a loop.
fn tsort_orders() -> Outcome {
    let checked = same(
        &shell_on("printf 'b c\\na b\\n' | tsort; printf 'a b b a' | tsort; echo $?")?,
        &ran(
            0,
            b"a\nb\nc\ntsort: -: a loop of a b\na\nb\ntsort ended with 1\n1\n",
        ),
    );
    expect(checked, "tsort should keep the pairs' order and say a loop")?;
    let checked = same(
        &shell_on("printf 'x y z' | tsort -; echo $?; tsort a b")?,
        &ran(
            1,
            b"tsort: -: an item without the other of its pair\ntsort ended with 1\n1\n\
              tsort: sorts one file\ntsort ended with 1\n",
        ),
    );
    expect(
        checked,
        "tsort should refuse an item without its pair, and more than one file",
    )?;
    // Pairs of more than one message, all of which count.
    let checked = same(
        &shell_on("printf 'w%s w%s\\n' 10 11 11 12 12 13 13 14 14 15 15 16 16 17 17 18 18 19 19 20 20 21 21 22 22 23 23 24 24 25 25 26 26 27 27 28 28 29 29 30 30 31 31 32 32 33 33 34 34 35 35 36 36 37 37 38 38 39 | tsort")?,
        &ran(0, b"w10\nw11\nw12\nw13\nw14\nw15\nw16\nw17\nw18\nw19\nw20\nw21\nw22\nw23\nw24\nw25\nw26\nw27\nw28\nw29\nw30\nw31\nw32\nw33\nw34\nw35\nw36\nw37\nw38\nw39\n"),
    );
    expect(
        checked,
        "tsort should read every pair however long its input",
    )
}

/// xargs runs a program on the words of its input, so many at a time,
/// echo without one, a command for each line with -I, stops at -E's word,
/// writes each command with -t, and ends as POSIX says when a program
/// ends otherwise, is not there, or the input breaks.
fn xargs_runs_programs() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'a b\\nc\\n' | xargs -n 2 echo; printf 'x\\n' | xargs printf '<%s>\\n'\n\
             printf '1 2\\n' | xargs; printf 'a\\nSTOP\\nb\\n' | xargs -E STOP echo",
        )?,
        &ran(0, b"a b\nc\n<x>\n1 2\na\n"),
    );
    expect(
        checked,
        "xargs should run a program on its words, echo without one, and stop at -E's word",
    )?;
    // What follows the word comes in messages of its own, which xargs
    // reads no more.
    let checked = same(
        &shell_on(
            "printf 'a STOP w0 w1 w2 w3 w4 w5 w6 w7 w8 w9 w10 w11 w12 w13 w14 w15 w16 w17 w18 w19 w20 w21 w22 w23 w24 w25 w26 w27 w28 w29 w30 w31 w32 w33 w34 w35 w36 w37 w38 w39\\n' | xargs -E STOP echo",
        )?,
        &ran(0, b"a\n"),
    );
    expect(
        checked,
        "xargs should read nothing after -E's word, however much follows",
    )?;
    let checked = same(
        &shell_on("printf '1\\n' | xargs false; echo $?; printf '1\\n' | xargs nope; echo $?")?,
        &ran(
            0,
            b"xargs ended with 123\n123\n\
              xargs: nope: no such program\nxargs ended with 127\n127\n",
        ),
    );
    expect(
        checked,
        "xargs should end with 123 after a program that failed and 127 for one not there",
    )?;
    let checked = same(
        &shell_on(
            "printf 'a\\n' | xargs -t echo; printf 'x y\\n' | xargs -I % echo '[%]'\n\
             printf \"a 'b\\n\" | xargs echo; echo $?",
        )?,
        &ran(
            0,
            b"echo a\na\n[x y]\na\nxargs: a single quote without its other half\n\
              xargs ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "xargs should write each command with -t, put each line where -I says, and run what came before a broken quote",
    )?;
    let checked = same(
        &shell_on(
            "printf 'a b \\nc\\nd\\n' | xargs -L 1 echo; printf 'aaaa bbbb cccc\\n' | xargs -s 15 echo\n\
             printf 'a b c d\\n' | xargs -n 3 -s 10 -x echo; echo $?",
        )?,
        &ran(
            0,
            b"a b c\nd\naaaa bbbb\ncccc\n\
              xargs: a command cannot take what -n or -L asks within -s\nxargs ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "xargs should take so many lines with -L, so many bytes with -s, and stop with -x when they do not fit",
    )?;
    // A program that ends with 255 stops xargs at once.
    let checked = same(
        &shell_on("printf 'exit 255\\nexit 0\\n' | xargs -I % shell -c %; echo $?")?,
        &ran(
            0,
            b"xargs: shell ended so that xargs stops\nxargs ended with 124\n124\n",
        ),
    );
    expect(
        checked,
        "xargs should stop with 124 once a program ends with 255",
    )
}

/// The programs xargs starts read an input that has ended, write where
/// xargs writes, and a long input makes as many of them as it needs.
fn xargs_hands_its_streams() -> Outcome {
    // The programs xargs starts read an input that has ended, not the
    // keyboard, and what they say goes where xargs says it.
    let checked = same(
        &shell_on("printf '' | xargs wc; printf '/nope\\n' | xargs wc; echo $?")?,
        &ran(
            0,
            b"0 0 0\nwc: /nope: no such file\nxargs ended with 123\n123\n",
        ),
    );
    expect(
        checked,
        "the programs xargs starts should read an input that ended, and say what went wrong where xargs does",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    // A long input makes many commands, which together take every word.
    let (ended, said) = shell_on(
        "head -n 300 /System/Boot/meridian/boot.meri | wc -w\n\
         head -n 300 /System/Boot/meridian/boot.meri | xargs -n 7 -s 200 echo | wc -w",
    )?;
    let counts: Vec<u64> = String::from_utf8_lossy(&said)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    expect(
        ended == Ok(Ended::Exited(0))
            && counts.len() == 2
            && counts[0] > 7
            && counts[0] == counts[1],
        "xargs should run as many commands as the words of a long input need, losing none",
    )?;
    let (ended, said) = shell_on(
        "printf 'a b\\n' | xargs echo > /System/x.txt || exit 9\n\
         cat /System/x.txt; rm /System/x.txt",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/x.txt: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"a b\n",
        "what the programs xargs starts write should go into the file xargs writes to",
    )
}

/// echo, printf, test, true and false run as programs of their own,
/// started by their files rather than as the shell's own commands.
fn commands_as_programs() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let p = "/System/Boot/meridian/programs";
    let checked = same(
        &shell_on(&alloc::format!(
            "{p}/echo.mexec a b; {p}/echo.mexec\n\
             {p}/printf.mexec '%s-%s\\n' x y; {p}/test.mexec -d /System; echo $?\n\
             {p}/true.mexec; {p}/false.mexec; echo $?"
        ))?,
        &ran(
            0,
            alloc::format!("a b\n\nx-y\n0\n{p}/false.mexec ended with 1\n1\n").as_bytes(),
        ),
    );
    expect(
        checked,
        "echo, printf, test, true and false should run as programs of their own",
    )?;
    // What test as a program learns of files: a program's file may run,
    // other files not; the loader's volume is only read; a directory is
    // one; nothing is not there; and a pipe is no terminal.
    let test = alloc::format!("{p}/test.mexec");
    // The statuses echo writes, among what the shell says of programs
    // ending with something else than 0. A script of three keeps what
    // it says within the eight messages a check's shell may say.
    let statuses = |scripts: &[&str]| -> Result<String, &'static str> {
        let mut all = Vec::new();
        for script in scripts {
            let (ended, said) = shell_on(script)?;
            let said = String::from_utf8(said).map_err(|_| "the statuses should be text")?;
            if ended != Ok(Ended::Exited(0)) {
                let _ = crate::log!("the shell ended {:?} after {:?}", ended, said);
                return Err("the shell should end with 0");
            }
            all.extend(
                said.lines()
                    .filter(|line| line.parse::<u8>().is_ok())
                    .map(String::from),
            );
        }
        Ok(all.join(" "))
    };
    let rights = statuses(&[
        &alloc::format!(
            "{test} -x {p}/echo.mexec; echo $?; {test} -x /System/Boot/meridian/boot.meri; echo $?\n\
             {test} -r {p}/echo.mexec; echo $?"
        ),
        &alloc::format!(
            "{test} -w {p}/echo.mexec; echo $?; {test} -x {p}; echo $?; {test} -x /nope; echo $?"
        ),
    ])?;
    expect(
        rights == "0 1 0 1 0 1",
        "test as a program should find a program and a directory runnable, another file and nothing not, and the loader's volume only read",
    )?;
    let kinds = statuses(&[
        &alloc::format!("{test} -d {p}; echo $?; {test} -f {p}; echo $?; {test} -e /nope; echo $?"),
        &alloc::format!("{test} -t 1; echo $?; {test} -t 2; echo $?"),
        &alloc::format!("{test} -t 0; echo $?; {test} -t 5; echo $?"),
    ])?;
    expect(
        kinds == "0 1 1 1 1 0 1",
        "test as a program should know a directory, a file not there, a pipe that is no terminal, the console that is one, and no stream past 2",
    )?;
    // What went wrong goes to the program's standard error.
    let checked = same(
        &shell_on(&alloc::format!(
            "{test} x -eq 1; echo $?; {p}/printf.mexec; echo $?"
        ))?,
        &ran(
            0,
            alloc::format!(
                "test: x: not a whole number\n{test} ended with 2\n2\n\
                 printf: needs a format\n{p}/printf.mexec ended with 1\n1\n"
            )
            .as_bytes(),
        ),
    );
    expect(
        checked,
        "test and printf as programs should say what went wrong on their standard error",
    )?;
    // A file made where /System is written may be written, holds
    // something, and once removed is not there.
    let (ended, said) = shell_on(&alloc::format!(
        "printf x > /System/w.txt || exit 9\n\
         {test} -w /System/w.txt; echo $?; {test} -s /System/w.txt; echo $?\n\
         rm /System/w.txt; {test} -e /System/w.txt; echo $?"
    ))?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/w.txt: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == alloc::format!("0\n0\n{test} ended with 1\n1\n").as_bytes(),
        "test as a program should find a new file writable and not empty, and gone once removed",
    )
}

/// sort orders by the keys -k names, keeps the first of lines alike with
/// -u, says where its input is out of order with -c, writes into one of
/// its inputs with -o, and merges with -m.
fn sort_options() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'b 2\\na 10\\nc 1\\n' | sort -k 2n; printf 'a\\nB\\nb\\nA\\n' | sort -fu\n\
             printf 'b\\na\\n' | sort -c; sort -k 0",
        )?,
        &ran(
            2,
            b"c 1\nb 2\na 10\na\nB\nsort: -:2: disorder: a\nsort ended with 1\n\
              sort: 0 is no key\nsort ended with 2\n",
        ),
    );
    expect(
        checked,
        "sort should order by keys, keep the first of lines alike, and say where its input is out of order",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'b\\na\\n' > /System/s1 || exit 9\n\
         sort -o /System/s1 /System/s1; cat /System/s1\n\
         printf 'x\\n' | sort -m - /System/s1; rm /System/s1",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/s1: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"a\nb\na\nb\nx\n",
        "sort should write into the file it read, and merge its input with a file",
    )
}

/// split closes each file it made before it makes the next, so it makes
/// more than a program may hold open, and reads an input longer than a
/// piece whole.
fn split_makes_many_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/Sp2 || exit 9\n\
         cd /System/Sp2\n\
         printf '%s\\n' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64 65 66 67 68 69 70 | split -l 1\n\
         ls | wc -l; cat xcr\n\
         rm x*; cd /; rd /System/Sp2",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/Sp2: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"70\n70\n",
        "split should make seventy files of a line each, the last holding the last line",
    )
}

/// uniq writes only the runs with -d and only the lines alone with -u,
/// leaves fields and characters out of the comparison with -f and -s,
/// reads the standard input for -, and writes into an output file, which
/// a missing input leaves as it was.
fn uniq_options() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'a\\na\\nb\\nc\\nc\\n' | uniq -d; printf 'a\\na\\nb\\nc\\nc\\n' | uniq -cu\n\
             printf '1 x\\n2 x\\n3 y\\n' | uniq -f 1 -; printf 'xa\\nya\\n' | uniq -s 1; uniq -q",
        )?,
        &ran(
            2,
            b"a\nc\n1 b\n1 x\n3 y\nxa\nuniq: -q is no option\nuniq ended with 2\n",
        ),
    );
    expect(
        checked,
        "uniq should write runs or lines alone, and leave fields and characters out",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'keep\\n' > /System/u2 || exit 9\n\
         printf 'a\\na\\n' | uniq - /System/u1; cat /System/u1\n\
         uniq /nope /System/u2; cat /System/u2\n\
         rm /System/u1 /System/u2",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/u2: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"a\nuniq: /nope: no such file\nuniq ended with 2\nkeep\n",
        "uniq should write into an output file, and leave it as it was when its input is not there",
    )
}

/// split says what words it cannot take, and cuts its input into files of
/// so many lines or bytes, named after the name it is given.
fn split_cuts_files() -> Outcome {
    let checked = same(
        &shell_on("split -x; split -l 0; split a b c")?,
        &ran(
            1,
            b"split: -x is no option\nsplit ended with 1\n\
              split: -l wants a number from 1, not 0\nsplit ended with 1\n\
              split: takes a file and a name\nsplit ended with 1\n",
        ),
    );
    expect(checked, "split should say what words it cannot take")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/Sp || exit 9\n\
         cd /System/Sp\n\
         printf '1\\n2\\n3\\n4\\n5\\n' | split -l 2; cat xaa xab xac | wc -l; cat xac\n\
         printf 'abcdefg\\n' | split -b 3 - p; cat pab pac\n\
         printf 'a\\nb\\nc\\n' | split -l 1 -a 1 - n; ls | wc -l\n\
         rm xaa xab xac paa pab pac na nb nc; cd /; rd /System/Sp",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/Sp: on a volume that is only read\n"),
            "on /System read only no directory should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"5\n5\ndefg\n9\n",
        "split should cut its input into files of so many lines or bytes",
    )
}

/// join says what words it cannot take and what it cannot open, pairs the
/// lines of two sorted files, writes those that pair with none and the
/// fields -o names, reads the standard input for -, and says when a file
/// is out of order.
fn join_pairs_lines() -> Outcome {
    let checked = same(
        &shell_on("join a; join -x a b; printf 'a 1\\n' | join - /nope")?,
        &ran(
            1,
            b"join: joins two files\njoin ended with 1\n\
              join: -x is no option\njoin ended with 1\n\
              join: /nope: no such file\njoin ended with 1\n",
        ),
    );
    expect(
        checked,
        "join should say what words it cannot take and what it cannot open",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "printf 'a 1\\nb 2\\nb 3\\nd 4\\n' > /System/j1 || exit 9\n\
         printf 'a x\\nb y\\nc w\\n' > /System/j2\n\
         join /System/j1 /System/j2; join -a 1 -a 2 -o 0,1.2,2.2 -e - /System/j1 /System/j2\n\
         printf 'b z\\n' | join -v 1 /System/j1 -; printf 'b\\na\\n' | join - /System/j2\n\
         rm /System/j1 /System/j2",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/j1: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"a 1 x\nb 2 y\nb 3 y\n\
                     a 1 x\nb 2 y\nb 3 y\nc - w\nd 4 -\n\
                     a 1\nd 4\n\
                     b y\njoin: - is not sorted on its join field\njoin ended with 1\n",
        "join should pair lines, write those that pair with none, and say a file is out of order",
    )
}

/// expand turns tabs into spaces up to the stops, every eight columns or
/// as -t says, and unexpand turns the blanks a line starts with back into
/// tabs, or with -a every run of two blanks or more, from files and from
/// the standard input.
fn expand_and_unexpand() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'a\\tb\\n' | expand; printf 'a\\tb\\tc\\td\\n' | expand -t 2,5\n\
             printf '        a  b       c\\n' | unexpand | od -c\n\
             printf '        a  b       c\\n' | unexpand -a | od -c\n\
             expand -t 0",
        )?,
        &ran(
            1,
            b"a       b\na b  c d\n\
              0000000  \\t   a           b                               c  \\n\n0000016\n\
              0000000  \\t   a           b  \\t               c  \\n\n0000013\n\
              expand: 0 is no tab stop\nexpand ended with 1\n",
        ),
    );
    expect(
        checked,
        "expand and unexpand should turn tabs into spaces and spaces into tabs",
    )?;
    let checked = same(
        &shell_on(
            "printf '\\ta\\n' | expand -t 3 - /nope; echo $?; unexpand -q; printf '    x\\n' | unexpand -t 2 | od -c",
        )?,
        &ran(
            0,
            b"expand: /nope: no such file\n   a\nexpand ended with 1\n1\n\
              unexpand: -q is no option\nunexpand ended with 1\n\
              0000000  \\t  \\t   x  \\n\n0000004\n",
        ),
    );
    expect(
        checked,
        "expand should read the standard input for -, and say what it could not read; unexpand -t should set its stops",
    )?;
    let checked = same(
        &shell_on("unexpand /nope; echo $?")?,
        &ran(
            0,
            b"unexpand: /nope: no such file\nunexpand ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "unexpand should end with 1 for what it could not read",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    // A file longer than one piece, 26 lines of 12 bytes, comes through
    // whole.
    let (ended, said) = shell_on(
        "printf '%s\\n' aaaaaaaaaaa bbbbbbbbbbb ccccccccccc ddddddddddd eeeeeeeeeee fffffffffff ggggggggggg hhhhhhhhhhh iiiiiiiiiii jjjjjjjjjjj kkkkkkkkkkk lllllllllll mmmmmmmmmmm nnnnnnnnnnn ooooooooooo ppppppppppp qqqqqqqqqqq rrrrrrrrrrr sssssssssss ttttttttttt uuuuuuuuuuu vvvvvvvvvvv wwwwwwwwwww xxxxxxxxxxx yyyyyyyyyyy zzzzzzzzzzz > /System/e.txt || exit 9\n\
         wc -l < /System/e.txt; expand /System/e.txt | wc -l; unexpand /System/e.txt | wc -l\n\
         rm /System/e.txt",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/e.txt: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"26\n26\n26\n",
        "expand and unexpand should read a whole file, longer than a piece",
    )
}

/// wc counts only what its options ask for, in its own order, characters
/// apart from bytes, and refuses an option it does not know.
fn wc_counts_what_it_is_asked() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'été a\\nb\\n' | wc -m; printf 'été a\\nb\\n' | wc -c -l\n\
             printf 'a b\\nc\\n' | wc -wl; wc -x",
        )?,
        &ran(1, b"8\n2 10\n2 3\nwc: -x is no option\nwc ended with 1\n"),
    );
    expect(
        checked,
        "wc should count what its options ask for, characters apart from bytes",
    )
}

/// od writes two-byte octal words unless told otherwise, lines up the
/// types it is given, reads on past a file it cannot open, skips and
/// stops where it is told, and refuses what it cannot do.
fn od_dumps_bytes() -> Outcome {
    let checked = same(
        &shell_on("printf 'abc\\n' | od; printf 'ab\\n' | od -A n -t x1 -c; od -t x3")?,
        &ran(
            1,
            b"0000000 061141 005143\n0000004\n\
              \x20 61  62  0a\n   a   b  \\n\n\
              od: numbers are 1, 2, 4 or 8 bytes, not 3\nod ended with 1\n",
        ),
    );
    expect(
        checked,
        "od should write octal words, line up two types, and refuse a size no number has",
    )?;
    let checked = same(
        &shell_on("printf 'ab' | od -c /nope -; printf 'ab' | od -j 3; echo $?")?,
        &ran(
            0,
            b"od: /nope: no such file\n0000000   a   b\n0000002\nod ended with 1\n\
              od: -j skips past the end of the input\nod ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "od should read on past a file it cannot open, and say when it cannot skip so far",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let program = "/System/Boot/meridian/programs/od.mexec";
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "od -c -N 4 {program}; od -A x -t x2 -j 16 -N 4 {program}\n\
         wc < {boot}; od -A d -c {boot} | tail -n 1"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    // wc writes the lines, the words and then the bytes.
    let bytes = lines
        .get(4)
        .and_then(|l| l.split_whitespace().nth(2)?.parse::<u64>().ok());
    let end = lines.get(5).and_then(|l| l.parse::<u64>().ok());
    expect(
        ended == Ok(Ended::Exited(0))
            && lines.len() == 6
            && lines[..4]
                == [
                    "0000000 177   E   L   F",
                    "0000004",
                    "000010 0003 003e",
                    "000014",
                ]
            && bytes.is_some_and(|bytes| bytes > 16)
            && bytes == end,
        "od should read a program's first bytes, skip to its type and machine, and end a whole file at its size",
    )?;
    // A directory opens but gives no bytes, which od says and ends with
    // 1 for, after reading on.
    let checked = same(
        &shell_on("od /System/Boot; echo $?")?,
        &ran(
            0,
            b"od: /System/Boot: a directory, not a file\n0000000\nod ended with 1\n1\n",
        ),
    );
    expect(
        checked,
        "od should say what it could not read and end with 1",
    )
}

/// basename and dirname take a path apart, and tr changes, deletes and
/// squeezes what it reads, as POSIX's do, in a sh script too.
fn basename_dirname_and_tr() -> Outcome {
    let checked = same(
        &shell_on(
            "basename /a/b/notes.txt .txt; dirname /a/b/notes.txt\n\
             echo 'Hello,  World' | tr a-z A-Z | tr -s ' '\n\
             echo education | tr -d aeiou\n\
             echo one..two | tr -cs a-z '\\n'",
        )?,
        &ran(0, b"notes\n/a/b\nHELLO, WORLD\ndctn\none\ntwo\n"),
    );
    expect(checked, "basename, dirname and tr should do as POSIX's do")?;
    let checked = same(
        &shell_on("#!/bin/sh\nf=/a/b/c.sh; echo \"$(dirname $f) $(basename $f .sh)\"; tr x")?,
        &ran(
            2,
            b"/a/b c\ntr: takes SET1 and SET2, or SET1 alone with -d or -s\ntr ended with 2\n",
        ),
    );
    expect(
        checked,
        "the tools should run in sh scripts and say what they need",
    )?;
    let checked = same(
        &shell_on("basename; dirname a b; basename -- -x .x; dirname -- -x/y")?,
        &ran(
            0,
            b"basename: takes a path, and a suffix to take off\nbasename ended with 2\n\
              dirname: takes one path\ndirname ended with 2\n-x\n-x\n",
        ),
    );
    expect(
        checked,
        "basename and dirname should say what they take, and take -- first",
    )
}

/// tr changes a file longer than one message through, line by line the
/// same as it was, and deleting the line ends leaves the bytes but them.
fn tr_on_a_long_file() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let shim = "/System/Boot/meridian/licences/shim.txt";
    let (ended, said) = shell_on(&alloc::format!(
        "wc < {shim}; tr a-z A-Z < {shim} | wc; tr -d '\\n' < {shim} | wc"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let counts: Vec<Vec<u64>> = text
        .lines()
        .map(|line| line.split(' ').filter_map(|n| n.parse().ok()).collect())
        .collect();
    let same_counts = counts
        .first()
        .is_some_and(|whole| counts.get(1) == Some(whole));
    let without_ends = match (counts.first(), counts.get(2)) {
        (Some(whole), Some(joined)) => {
            whole.len() == 3
                && joined.len() == 3
                && joined[0] == 0
                && joined[2] == whole[2] - whole[0]
                && whole[2] > 4096
        }
        _ => false,
    };
    expect(
        ended == Ok(Ended::Exited(0)) && counts.len() == 3 && same_counts && without_ends,
        "tr should go through a file longer than one message",
    )
}

/// cut takes the fields, characters and bytes of each line it is told,
/// and tee writes what it reads to its output and into files, anew or at
/// their end.
fn cut_and_tee() -> Outcome {
    let checked = same(
        &shell_on(
            "printf 'a:b:c\\nx:y\\n' | cut -d : -f 2\n\
             echo hello | cut -c 2-4; echo hello | cut -b -2\n\
             echo one | tee | tee",
        )?,
        &ran(0, b"b\ny\nell\nhe\none\n"),
    );
    expect(
        checked,
        "cut should take the parts it is told, and tee hand on its input",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo first | tee /System/tee.txt || exit 9\n\
         echo more | tee -a /System/tee.txt | wc\n\
         cat /System/tee.txt; rm /System/tee.txt",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"tee: /System/tee.txt: on a volume that is only read\n"),
            "on /System read only tee should say it cannot write the file",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"first\n1 1 5\nfirst\nmore\n",
        "tee should write into a file anew, and at its end with -a",
    )
}

/// tee says which file it could not open, and still writes its input to
/// its output and to the files it could; it ends with 1 for that. It
/// writes into several files at once.
fn tee_where_it_cannot_write() -> Outcome {
    let checked = same(
        &shell_on("echo line | tee /nope/x /nope/y")?,
        &ran(
            1,
            b"tee: /nope/x: no such file\ntee: /nope/y: no such file\nline\ntee ended with 1\n",
        ),
    );
    expect(
        checked,
        "tee should say which file it could not open, and go on",
    )?;
    let checked = same(
        &shell_on("tee -x")?,
        &ran(2, b"tee: -x is no option\ntee ended with 2\n"),
    );
    expect(checked, "tee should refuse an option it does not know")?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo both | tee /System/tee1.txt /System/tee2.txt > /System/tee3.txt || exit 9\n\
         cat /System/tee1.txt /System/tee2.txt /System/tee3.txt\n\
         rm /System/tee1.txt /System/tee2.txt /System/tee3.txt",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/tee3.txt: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"both\nboth\nboth\n",
        "tee should write into every file it names and its output",
    )
}

/// cut goes through each file it names in turn, says which one it could
/// not read, and ends with 1 for that; a file it reads it cuts as it would
/// the same text on its input.
fn cut_on_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let boot = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!(
        "cut -c 1-3 /nope {boot}\necho .\ncut -c 1-3 < {boot}"
    ))?;
    let text = core::str::from_utf8(&said).unwrap_or("");
    let read = text
        .strip_prefix("cut: /nope: no such file\n")
        .and_then(|rest| rest.split_once("cut ended with 1\n.\n"));
    let Some((from_file, from_input)) = read else {
        return expect(false, "cut should say which file it could not read");
    };
    expect(
        ended == Ok(Ended::Exited(0))
            && !from_file.is_empty()
            && from_file == from_input
            && from_file.lines().all(|line| line.chars().count() <= 3),
        "cut should cut a file as it cuts the same text on its input",
    )
}

/// tee writes a file it finds anew, so nothing of a longer old text stays
/// behind, and with -a makes a file that is not there yet.
fn tee_writes_anew() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo a longer first line > /System/tee4.txt || exit 9\n\
         echo x | tee -- /System/tee4.txt\n\
         cat /System/tee4.txt\n\
         echo y | tee -a /System/tee5.txt\n\
         cat /System/tee5.txt\n\
         rm /System/tee4.txt /System/tee5.txt",
    )?;
    if ended == Ok(Ended::Exited(9)) {
        return expect(
            said.starts_with(b"/System/tee4.txt: on a volume that is only read\n"),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"x\nx\ny\ny\n",
        "tee should write a file anew, and make one with -a",
    )
}

/// cut refuses a list or options it cannot use and ends with 2 before it
/// reads anything. With -s it leaves out the lines with no delimiter, and
/// -c counts characters where -b counts bytes.
fn cut_refuses_and_leaves_out() -> Outcome {
    let checked = same(
        &shell_on("cut -c 0; cut -f 3-1; cut -d ab -f 1; cut -s -c 1")?,
        &ran(
            2,
            b"cut: parts are counted from 1\ncut ended with 2\n\
              cut: 3-1 goes down\ncut ended with 2\n\
              cut: -d takes one character\ncut ended with 2\n\
              cut: -d and -s go with -f\ncut ended with 2\n",
        ),
    );
    expect(checked, "cut should refuse what it cannot cut by")?;
    let checked = same(
        &shell_on(
            "printf 'a,b\\nab\\n,\\n' | cut -s -d , -f 2\n\
             echo über | cut -c 1; echo über | cut -b 1-2",
        )?,
        &ran(0, "b\n\nü\nü\n".as_bytes()),
    );
    expect(
        checked,
        "cut -s should leave out lines with no delimiter, and -c count characters",
    )
}

/// What `help` shows, counted as wc counts.
const HELP_WORDS: usize = 67;
const HELP_BYTES: usize = 309;

fn writes_on_the_console() -> Outcome {
    // The first piece would end inside the last character. Spaces, since
    // they take the console no time to draw.
    let long = alloc::format!("{}\u{e4}\n", " ".repeat(abi::WRITE_BYTES - 1));
    expect(
        write(&long) == Ok(()),
        "a text longer than one call takes should go in pieces cut between characters",
    )?;
    expect(
        write("hello on the console\n") == Ok(()),
        "a line should go to the console",
    )?;
    let broken = [0xffu8, 0xfe];
    let not_text = abi::Call::Write {
        len: broken.len() as u64,
        at: broken.as_ptr() as u64,
    };
    let too_long = abi::Call::Write {
        len: abi::WRITE_BYTES as u64 + 1,
        at: broken.as_ptr() as u64,
    };
    let kernels = abi::Call::Write {
        len: 1,
        at: abi::USER_END,
    };
    expect(
        abi::answer(raw::call(not_text).0) == Err(Error::BadArgument)
            && abi::answer(raw::call(too_long).0) == Err(Error::TooLong)
            && abi::answer(raw::call(kernels).0) == Err(Error::BadAddress),
        "bytes that are no UTF-8, more than WRITE_BYTES and the kernel's half should not be written",
    )?;
    expect(
        key().is_none(),
        "no key should wait before anyone pressed one",
    )
}

fn wait_for_no_process() -> Outcome {
    let (a, b) = pair()?;
    expect(
        wait(a) == Err(Error::Denied),
        "a channel end is no process to wait for",
    )?;
    expect(
        wait(Handle(0x7777_0042)) == Err(Error::BadHandle),
        "nor is a made up handle",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn not_a_handle() -> Outcome {
    let mut room = [0u8; 4];
    let stranger = Handle(0x7777_0042);
    expect(
        send(stranger, b"x", None) == Err(Error::BadHandle)
            && receive(stranger, &mut room) == Err(Error::BadHandle)
            && close(stranger) == Err(Error::BadHandle),
        "a handle the program never got should reach nothing",
    )
}
