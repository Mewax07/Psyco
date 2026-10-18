use psycoc::{Target, compile_str};

#[track_caller]
fn same(sugar: &str, by_hand: &str) {
    for target in [Target::Linux, Target::Uefi] {
        let a = compile_str(sugar, target).unwrap_or_else(|e| panic!("sugar: {e}"));
        let b = compile_str(by_hand, target).unwrap_or_else(|e| panic!("by hand: {e}"));
        assert!(
            a == b,
            "different code ({} vs {} bytes)\n--- sugar ---\n{sugar}\n--- by hand ---\n{by_hand}",
            a.len(),
            b.len()
        );
    }
}

#[test]
fn getter_and_setter_are_direct_accesses() {
    same(
        "#[getter, setter]\nstatic mut ticks: u64 = 0\nfn main() { set_ticks(get_ticks() + 1); print(get_ticks()) }",
        "static mut ticks: u64 = 0\nfn main() { ticks = ticks + 1; print(ticks) }",
    );
}

#[test]
fn array_const_is_an_immutable_static() {
    same(
        "const t: [u16; 3] = [1, 2, 3]\nfn main() { print(t[2], t.len()) }",
        "static t: [u16; 3] = [1, 2, 3]\nfn main() { print(t[2], t.len()) }",
    );
}

#[test]
fn inferred_array_literal_matches_suffixed_one() {
    same(
        "fn f(b: &[u8]) -> u8 { return b[1] }\nfn main() { print(f(&[0x23, 7, 255])) }",
        "fn f(b: &[u8]) -> u8 { return b[1] }\nfn main() { print(f(&[0x23u8, 7u8, 255u8])) }",
    );
}

#[test]
fn generic_function_matches_hand_written_one() {
    same(
        "fn max<T>(a: T, b: T) -> T { if a > b { return a } return b }\nfn main() { print(max(3u8, 9)) }",
        "fn main() { print(max_u8(3u8, 9)) }\nfn max_u8(a: u8, b: u8) -> u8 { if a > b { return a } return b }",
    );
}

#[test]
fn generic_struct_matches_hand_written_one() {
    same(
        "struct Pair<A, B> { a: A, b: B }\nfn main() { let p = Pair { a: 1u8, b: 2u64 }; print(p.a, p.b, sizeof(Pair<u8, u64>)) }",
        "struct PairU8U64 { a: u8, b: u64 }\nfn main() { let p = PairU8U64 { a: 1u8, b: 2u64 }; print(p.a, p.b, sizeof(PairU8U64)) }",
    );
}

#[test]
fn generic_method_matches_hand_written_one() {
    same(
        "struct Cell<T> { v: T }\nimpl<T> Cell<T> { fn get(&self) -> T { return self.v } }\nfn main() { let c = Cell { v: 5u32 }; print(c.get()) }",
        "struct Cell { v: u32 }\nfn main() { let c = Cell { v: 5u32 }; print(c.get()) }\nimpl Cell { fn get(&self) -> u32 { return self.v } }",
    );
}

#[test]
fn trait_call_is_a_direct_call() {
    same(
        "trait Area { fn area(&self) -> u64 }\nstruct Sq { s: u64 }\nimpl Area for Sq { fn area(&self) -> u64 { return self.s * self.s } }\nfn show<T: Area>(x: &T) { print(x.area()) }\nfn main() { show(&Sq { s: 3 }) }",
        "struct Sq { s: u64 }\nfn main() { show(&Sq { s: 3 }) }\nfn show(x: &Sq) { print(area_sq(x)) }\nfn area_sq(x: &Sq) -> u64 { return x.s * x.s }",
    );
}
