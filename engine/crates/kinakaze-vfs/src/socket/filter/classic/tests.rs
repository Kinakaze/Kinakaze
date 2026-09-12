use super::*;
use crate::socket::filter::LocalPacket;
fn i(code: u16, k: u32) -> Instruction {
    Instruction {
        code,
        k,
        jt: 0,
        jf: 0,
    }
}
fn run(code: Vec<Instruction>, bytes: &[u8]) -> u32 {
    Program::new(code).unwrap().run(&LocalPacket(bytes))
}

#[test]
fn complete_alu_matrix_uses_unsigned_wrapping_arithmetic() {
    let cases = [
        (0x00, 0xfffffffdu32.wrapping_add(3)),
        (0x10, 0xfffffffdu32.wrapping_sub(3)),
        (0x20, 0xfffffffdu32.wrapping_mul(3)),
        (0x30, 0xfffffffdu32 / 3),
        (0x40, 0xfffffffdu32 | 3),
        (0x50, 0xfffffffdu32 & 3),
        (0x60, 0xfffffffdu32 << 3),
        (0x70, 0xfffffffdu32 >> 3),
        (0x90, 0xfffffffdu32 % 3),
        (0xa0, 0xfffffffdu32 ^ 3),
    ];
    for (op, expected) in cases {
        for source in [0, 8] {
            assert_eq!(
                run(
                    vec![i(0, 0xfffffffd), i(1, 3), i(op | 4 | source, 3), i(0x16, 0)],
                    &[]
                ),
                expected,
                "op={op:x} source={source}"
            );
        }
    }
    assert_eq!(run(vec![i(0, 1), i(0x84, 0), i(0x16, 0)], &[]), u32::MAX);
    for code in [0x3c, 0x9c] {
        assert_eq!(run(vec![i(0, 10), i(code, 0), i(6, 99)], &[]), 0);
    }
    assert_eq!(run(vec![i(0, 1), i(1, 32), i(0x6c, 0), i(0x16, 0)], &[]), 1);
}

#[test]
fn validates_every_opcode_and_all_forward_targets() {
    for code in 0..=u16::MAX {
        let accepted = matches!(
            code,
            0x00 | 0x01
                | 0x80
                | 0x81
                | 0xb1
                | 0x07
                | 0x87
                | 0x06
                | 0x16
                | 0x20
                | 0x28
                | 0x30
                | 0x40
                | 0x48
                | 0x50
                | 0x02
                | 0x03
                | 0x04
                | 0x0c
                | 0x14
                | 0x1c
                | 0x24
                | 0x2c
                | 0x34
                | 0x3c
                | 0x44
                | 0x4c
                | 0x54
                | 0x5c
                | 0x64
                | 0x6c
                | 0x74
                | 0x7c
                | 0x84
                | 0x94
                | 0x9c
                | 0xa4
                | 0xac
                | 0x15
                | 0x1d
                | 0x25
                | 0x2d
                | 0x35
                | 0x3d
                | 0x45
                | 0x4d
        );
        assert_eq!(
            Program::new(vec![i(code, 1), i(6, 0)]).is_ok(),
            accepted,
            "{code:x}"
        );
    }
    assert!(Program::new(vec![i(5, 0), i(6, 1)]).is_ok());
    for k in [1, u32::MAX] {
        assert!(Program::new(vec![i(5, k), i(6, 1)]).is_err());
    }
    let branch = Instruction {
        code: 0x15,
        k: 0,
        jt: 1,
        jf: 0,
    };
    assert!(Program::new(vec![branch, i(6, 1)]).is_err());
    for code in [0x34, 0x94] {
        assert!(Program::new(vec![i(code, 0), i(6, 1)]).is_err());
    }
    for code in [0x64, 0x74] {
        assert!(Program::new(vec![i(code, 32), i(6, 1)]).is_err());
    }
    assert!(Program::new(vec![]).is_err());
    assert!(Program::new(vec![i(6, 1); 4097]).is_err());
    assert!(Program::new(vec![i(6, 1); 4096]).is_ok());
    assert!(Program::new(vec![i(0, 1)]).is_err());
}

#[test]
fn scratch_words_must_be_initialized_on_every_path() {
    for slot in 0..16 {
        for (store, load) in [(2, 0x60), (3, 0x61)] {
            assert!(Program::new(vec![i(load, slot), i(6, 1)]).is_err());
            assert!(Program::new(vec![i(store, slot), i(load, slot), i(6, 1)]).is_ok());
            let branch = Instruction {
                code: 0x15,
                k: 0,
                jt: 1,
                jf: 0,
            };
            assert!(Program::new(vec![branch, i(store, slot), i(load, slot), i(6, 1)]).is_err());
        }
    }
    for code in [2, 3, 0x60, 0x61] {
        assert!(Program::new(vec![i(code, 16), i(6, 1)]).is_err());
    }
    assert_eq!(
        run(
            vec![
                i(0, 123),
                i(2, 15),
                i(0, 0),
                i(0x60, 15),
                i(7, 0),
                i(0x87, 0),
                i(0x16, 0)
            ],
            &[]
        ),
        123
    );
}

#[test]
fn packet_loads_are_big_endian_and_faults_drop_before_return() {
    let data = [0x45, 0x23, 0x67, 0x89, 0xab];
    for (code, value) in [(0x20, 0x45236789), (0x28, 0x4523), (0x30, 0x45)] {
        assert_eq!(run(vec![i(code, 0), i(0x16, 0)], &data), value);
        assert_eq!(run(vec![i(code, 5), i(6, 99)], &data), 0);
        assert_eq!(
            run(vec![i(code, (-0x100000i32) as u32), i(6, 99)], &data),
            0
        );
    }
    assert_eq!(run(vec![i(1, 1), i(0x48, 1), i(0x16, 0)], &data), 0x6789);
    assert_eq!(run(vec![i(0xb1, 0), i(0x87, 0), i(0x16, 0)], &data), 20);
    assert_eq!(run(vec![i(0x80, 0), i(0x16, 0)], &data), 5);
    assert_eq!(run(vec![i(0x81, 0), i(0x87, 0), i(0x16, 0)], &data), 5);
}

#[test]
fn conditional_jumps_compare_unsigned_and_test_bits() {
    for (code, rhs, result) in [
        (0x15, u32::MAX, 11),
        (0x25, 1, 11),
        (0x35, u32::MAX, 11),
        (0x45, 2, 11),
        (0x15, 0, 22),
    ] {
        for source in [0, 8] {
            let branch = Instruction {
                code: code | source,
                k: rhs,
                jt: 0,
                jf: 1,
            };
            assert_eq!(
                run(
                    vec![i(0, u32::MAX), i(1, rhs), branch, i(6, 11), i(6, 22)],
                    &[]
                ),
                result
            );
        }
    }
}

#[test]
fn ancillary_validation_xor_and_netlink_attribute_search() {
    for offset in [8, 28] {
        assert_eq!(
            run(vec![i(0, 7), i(0x20, AD_OFF + offset), i(6, 99)], &[]),
            7
        );
    }
    for offset in [12, 16] {
        assert_eq!(
            run(
                vec![i(0, u32::MAX), i(0x20, AD_OFF + offset), i(6, 99)],
                &[]
            ),
            99
        );
    }
    for offset in 0..64 {
        assert_eq!(
            Program::new(vec![i(0x20, AD_OFF + offset), i(6, 1)]).is_ok(),
            offset % 4 == 0
        );
    }
    assert!(Program::new(vec![i(0x20, AD_OFF + 64), i(6, 1)]).is_err());
    assert_eq!(
        run(
            vec![i(0, 0xf0), i(1, 0x33), i(0x20, AD_OFF + 40), i(0x16, 0)],
            &[]
        ),
        0xc3
    );
    let attributes = [0u8, 0, 0, 0, 12, 0, 1, 128, 8, 0, 7, 0, 1, 2, 3, 4];
    assert_eq!(
        run(
            vec![i(0, 4), i(1, 1), i(0x20, AD_OFF + 12), i(0x16, 0)],
            &attributes
        ),
        4
    );
    assert_eq!(
        run(
            vec![i(0, 4), i(1, 7), i(0x20, AD_OFF + 16), i(0x16, 0)],
            &attributes
        ),
        8
    );
    let program = Program::new(vec![i(6, u32::MAX)]).unwrap();
    assert_eq!(
        Program::decode(&program.encode()).unwrap().instructions(),
        program.instructions()
    );
}
