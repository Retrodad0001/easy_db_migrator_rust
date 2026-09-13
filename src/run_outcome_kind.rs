pub(crate) enum RunOutcomeKind {
    Completed { all_succeeded: bool },
    Cancelled,
}
