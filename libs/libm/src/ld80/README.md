The vendored powl.c is the musl v1.2.5 80-bit implementation, originally from OpenBSD/Stephen L. Moshier. Its permissive license is retained at the top of the source.

Source: https://raw.githubusercontent.com/ifduyue/musl/v1.2.5/src/math/powl.c
SHA-256: b1c9f5993fe37ce04e4291cdeaa8d690587f25076525a417c9657f082b7ae987

libm.h supplies the small private math helpers. bridge.c uses pointer arguments so Windows long-double return conventions never cross the ELF boundary; the Rust entry loads ST(0) explicitly. Clang compiles this with 80-bit long doubles and strict floating-point semantics.
