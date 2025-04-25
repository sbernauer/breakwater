# How to release

1. Grep for old version (e.g. `grep -r --exclude-dir target '0.17.0'`) and update as needed
2. Push commit `Release 0.18.0`
3. Create GitHub release (and tag), use the changelog as description
4. `cargo publish -p breakwater-parser`
5. `cargo publish -p breakwater-egui-overlay`
6. `cargo publish -p breakwater`
