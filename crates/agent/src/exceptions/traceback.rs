//! Python tracebacks on stderr: what each line does to the traceback in progress. Pure.

/// Where a Python traceback is: frames still coming, the exception line seen, or a
/// "During handling of the above exception…" line announcing a chained traceback.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Traceback {
    No,
    Frames,
    Done,
    Chained,
}

/// What a stderr line does to a Python traceback in progress.
pub enum Step {
    Start,
    Join,
    Skip,
    Summary,
    Chain,
    Pass,
}

const HEADER: &str = "Traceback (most recent call last)";
const CHAINS: [&str; 2] = [
    "During handling of the above exception, another exception occurred",
    "The above exception was the direct cause of the following exception",
];

/// What `line` does, given the record in progress: its traceback state and whether its
/// stack is still empty.
pub fn step(pending: Option<(Traceback, bool)>, line: &str) -> Step {
    let text = line.trim();
    let indented = line.starts_with([' ', '\t']);
    let Some((traceback, empty)) = pending else {
        return if text.starts_with(HEADER) { Step::Start } else { Step::Pass };
    };
    let logged = traceback == Traceback::No && empty;
    match traceback {
        _ if text.starts_with(HEADER) && (logged || traceback == Traceback::Chained) => Step::Join,
        _ if text.starts_with(HEADER) => Step::Start,
        Traceback::Frames | Traceback::Done | Traceback::Chained if text.is_empty() => Step::Skip,
        Traceback::Frames if !indented => Step::Summary,
        Traceback::Done if CHAINS.iter().any(|c| text.starts_with(c)) => Step::Chain,
        _ => Step::Pass,
    }
}
