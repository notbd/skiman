//! `skiman adopt <name> --from <tool id>`: bring a foreign skill in.

use crate::Exit;

pub fn run(_name: &str, _from: &str, _yes: bool) -> Exit {
    Exit::OperationalError
}
