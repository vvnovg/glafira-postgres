# Contributing

Thank you for your interest in Glafira for PostgreSQL.

## Before you start

- Open an issue first for anything bigger than a small fix, so that the approach can be agreed on.
- The working language of the repository is Russian: comments, commit messages and `docs/` are written in Russian (except the English guide in `docs/guide/en/` and the files for the releases repository in `docs/releases-repo/`).
  Issues and pull requests in English are welcome.

## Building and testing

See `README.md` («Требования», «Запуск», «Тесты»). Before opening a pull request run:

```bash
pnpm test && pnpm test:scripts && pnpm i18n:check && pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check
cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/pg-advisor/Cargo.toml
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses
cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
```

Integration tests need the Docker stand described in `README.md` («Стенд»). `cargo-deny` can be installed with
`cargo install cargo-deny --locked --version 0.20.2`.

## Interface strings and the guide

Every new string goes to both dictionaries in `src/locales/` (`en.json` and `ru.json`) at once (see
`docs/i18n/README.md` and `docs/i18n/glossary.md`); `pnpm i18n:check` enforces it. Changes to the user guide go to
both languages in `docs/guide/`.

## Developer Certificate of Origin

Contributions are accepted under the Apache License 2.0 (`LICENSE`). Commits in pull requests from contributors
must be signed off (`git commit -s`), which certifies the
[Developer Certificate of Origin](https://developercertificate.org/). Pull requests whose commits lack
a `Signed-off-by` line are not merged.
