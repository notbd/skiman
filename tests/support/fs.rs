use std::path::Path;

/// Asserts that `link` is a symlink whose link text is exactly `lexical`
/// and whose realpath equals the realpath of `resolved`.
#[track_caller]
pub fn assert_symlink(
    link: impl AsRef<Path>,
    lexical: impl AsRef<Path>,
    resolved: impl AsRef<Path>,
) {
    let (link, lexical, resolved) = (link.as_ref(), lexical.as_ref(), resolved.as_ref());
    let actual = std::fs::read_link(link)
        .unwrap_or_else(|e| panic!("{} is not a symlink: {e}", link.display()));
    assert_eq!(
        actual,
        lexical,
        "lexical target of {} differs",
        link.display()
    );
    let actual_real = link
        .canonicalize()
        .unwrap_or_else(|e| panic!("{} does not resolve: {e}", link.display()));
    let expected_real = resolved.canonicalize().unwrap_or_else(|e| {
        panic!(
            "expected target {} does not resolve: {e}",
            resolved.display()
        )
    });
    assert_eq!(
        actual_real,
        expected_real,
        "resolved target of {} differs",
        link.display()
    );
}

/// Asserts that `link` is a symlink whose link text is exactly `lexical`
/// and whose target does not exist.
#[track_caller]
pub fn assert_dangling_symlink(link: impl AsRef<Path>, lexical: impl AsRef<Path>) {
    let (link, lexical) = (link.as_ref(), lexical.as_ref());
    let actual = std::fs::read_link(link)
        .unwrap_or_else(|e| panic!("{} is not a symlink: {e}", link.display()));
    assert_eq!(
        actual,
        lexical,
        "lexical target of {} differs",
        link.display()
    );
    assert!(
        !link.exists(),
        "{} resolves to {:?}, expected a dangling link",
        link.display(),
        link.canonicalize()
    );
}
