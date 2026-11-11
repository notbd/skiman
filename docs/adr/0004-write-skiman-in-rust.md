# Write skiman in Rust

skiman ships as a single binary that starts in milliseconds, because it runs at the start of every agent session. Go and Rust both meet that; Rust wins on two points that matter here: several commands (`add`, `remove`, `mv`, `adopt`, `import`) edit the user's hand-written `skiman.toml`, and `toml_edit` preserves comments and layout where Go's equivalents are small projects; and a stricter compiler (exhaustive enums, no nil) catches more mistakes before tests do. Go's advantages, faster builds and easier reading, mostly help the development loop rather than the product.

## Considered Options

- **Go**: chosen first (over Python 3.12, spec v8's choice) before Rust was compared; simpler and faster to build, but weaker format-preserving TOML editing and a looser type system.
- **Python**: best format-preserving YAML editing, but needs a 3.12 runtime and starts slower; skiman only reads frontmatter and at most rewrites the `name` line.
