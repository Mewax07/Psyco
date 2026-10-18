use psycoc::{Target, compile_str};

#[track_caller]
fn bad(src: &str, fragment: &str) {
    match compile_str(src, Target::Linux) {
        Ok(_) => panic!("expected an error containing {fragment:?}\n--- program ---\n{src}"),
        Err(e) if e.contains(fragment) => {}
        Err(e) => panic!("expected an error containing {fragment:?}, got: {e}\n--- program ---\n{src}"),
    }
}

#[track_caller]
fn ok(src: &str) {
    if let Err(e) = compile_str(src, Target::Linux) {
        panic!("expected success, got: {e}\n--- program ---\n{src}");
    }
}

#[test]
fn no_use_after_free() {
    bad("fn main() { let mut v: Vec<u8> = Vec::new(); v.push(1); v.free(); let n = v.len() }", "use of moved value 'v'");
    bad("fn main() { let b = Box::new(1u8); b.free(); let x = b.get() }", "use of moved value 'b'");
    bad("fn main() { let s = String::from(\"a\"); s.free(); s.free() }", "use of moved value 's'");
}

#[test]
fn no_copies_of_owners() {
    bad("fn main() { let mut v: Vec<u8> = Vec::new(); let w = v; v.push(1) }", "use of moved value 'v'");
    bad("fn main() { let a = [String::new(); 2] }", "cannot repeat a String value");
    bad("fn f(v: &Vec<u8>) -> Vec<u8> { return *v }\nfn main() {}", "cannot move out of '*v'");
    bad("fn main() { let v: Vec<Vec<u8>> = Vec::new(); let x = v.get(0) }", "Vec<u8> does not implement 'Copy'");
    bad("struct W { v: Vec<u8> }\nfn main() { let w = W { v: Vec::new() }; let a = w.v; let b = w.v }", "use of moved value 'w'");
}

#[test]
fn borrows_of_heap_data() {
    bad("fn main() { let mut v: Vec<u8> = Vec::new(); v.push(1); let s = v.as_slice(); v.free(); let x = s[0] }", "cannot move 'v' while it is borrowed");
    bad("fn main() { let mut v: Vec<u8> = Vec::new(); v.push(1); let s = v.as_slice(); v.push(2); let x = s[0] }", "cannot borrow 'v' as mutable while it is borrowed");
    bad("fn main() { let x: u8 = 1; let b = Box::new(&x) }", "references cannot be stored");
    ok("fn main() { let mut v: Vec<u8> = Vec::new(); v.push(1); let s = v.as_slice(); let x = s[0]; v.push(x); v.free() }");
}

#[test]
fn heap_code_only_when_used() {
    let plain = compile_str("fn main() { print(1) }", Target::Linux).unwrap();
    let with_vec = compile_str("fn main() { let mut v: Vec<u8> = Vec::new(); v.push(1); print(v.len()); v.free() }", Target::Linux).unwrap();
    assert!(with_vec.len() > plain.len());
    assert!(compile_str("fn main() { let b = Box::new(1u8) }", Target::Uefi).unwrap_err().contains("psy_alloc"));
}
