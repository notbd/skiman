//! `skiman import --guide`: print the agent guide. Read-only.

use std::io::Write;

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::import_guide;

pub fn guide() -> Exit {
    finish(
        std::io::stdout()
            .write_all(import_guide::text().as_bytes())
            .map(|()| Exit::Success)
            .map_err(|e| Failure::operational(format!("cannot print the guide: {e}"))),
    )
}
