# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Confirmed: static HTML, CSS and JavaScript on GitHub Pages. A separate, dependency-free Rust bridge compiles the existing crate to WebAssembly. No frontend framework or build-time npm dependency is needed. The user chose code-first development with visual review at completion.

## Users

Rust developers evaluating and using audiobox for audio decoding, editing, analysis and encoding. Audience inferred from the library and the request for API documentation; no additional personas have been established.

## Product Purpose

Present audiobox in English, demonstrate it with real audio, provide runnable Rust examples, and explain the public API. A visitor should be able to install the crate, understand its limitations, process audio in the sandbox, and continue to the complete API reference.

## Operating Context

The Rust package is audiobox 0.1.0, published on crates.io. Its source is jmsansan/audiobox-rs. GitHub Pages will serve the public presentation, sandbox, guides and generated rustdoc. The TypeScript package lives in a separate repository.

## Capabilities and Constraints

The crate has no dependencies and forbids unsafe Rust. Audio data is immutable planar f32 PCM. Supported containers include WAV, AIFF, CAF, AU, FLAC and MP3, with the exact format matrix and limitations documented in README.md. MP3 encoding is MPEG-1 CBR; no VBR encoder or psychoacoustic model. Loudness range is an approximation. No invented speed comparisons, customers or benchmarks.

Confirmed sandbox: controls on real audio, with corresponding Rust code. The browser must execute actual Rust through WebAssembly, process files locally, play the result, display a waveform, and offer export. Browser sandbox limits may be stricter than the library's limits and must be explicit.

## Brand Commitments

Product name audiobox. English website. Clear technical copy, precise claims and working examples. No existing visual identity was found.

## Evidence on Hand

Public source in src/, README.md format matrix, 22 passing tests including FFmpeg interoperability, Cargo.toml package metadata, crates.io 0.1.0. Synthetic audio can be authored for the sandbox and must be labeled as demo audio.

## Product Principles

- Demonstrate the Rust implementation itself.
- Teach with compilable examples and accurate API signatures.
- State capabilities and limitations together.
- Keep install and exploration simple.
- Preserve the dependency-free crate.
