#[cfg(feature = "mssql")]
pub(crate) mod mssql;
#[cfg(feature = "postgres")]
pub(crate) mod postgres;

pub(crate) fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>, context: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{context}: {error:?}"),
    }
}

pub(crate) fn sweep_leftover_containers(test_label: &str) {
    let listed = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("label={test_label}=1")])
        .output();

    let Ok(listed) = listed else {
        return;
    };
    let Ok(ids) = String::from_utf8(listed.stdout) else {
        return;
    };

    for id in ids.split_whitespace() {
        eprintln!("removing container {id} left behind by an earlier run");
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a leftover container that cannot be removed does not change a test result"
        )]
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", id])
            .output();
    }
}
