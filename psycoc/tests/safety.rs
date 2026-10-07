use psycoc::{lexer::Lexer, parser::Parser, typeck::TypeChecker};

fn check(src: &str) -> Result<(), String> {
    let tokens = Lexer::new(src)
        .tokenize()
        .map_err(|e| format!("lexer: {}", e.message))?;
    let mut program = Parser::new(tokens)
        .parse_program()
        .map_err(|e| format!("parser {}:{}: {}", e.span.line, e.span.col, e.message))?;
    TypeChecker::new()
        .check_program(&mut program)
        .map(|_| ())
        .map_err(|e| format!("{}:{}: {}", e.span.line, e.span.col, e.message))
}

fn check_files(files: &[&str]) -> Result<(), String> {
    let mut program = psycoc::ast::Program::default();
    for src in files {
        let tokens = Lexer::new(src)
            .tokenize()
            .map_err(|e| format!("lexer: {}", e.message))?;
        let p = Parser::new(tokens)
            .parse_program()
            .map_err(|e| format!("parser {}:{}: {}", e.span.line, e.span.col, e.message))?;
        program.merge(p);
    }
    TypeChecker::new()
        .check_program(&mut program)
        .map(|_| ())
        .map_err(|e| format!("{}:{}: {}", e.span.line, e.span.col, e.message))
}

#[track_caller]
fn trusted_ok(body: &str) {
    let src = format!("#![trusted]\n{PRELUDE}\n{body}");
    if let Err(e) = check(&src) {
        panic!("expected success, got error: {e}\n--- program ---\n{body}");
    }
}

#[track_caller]
fn ok(body: &str) {
    let src = format!("{PRELUDE}\n{body}");
    if let Err(e) = check(&src) {
        panic!("expected success, got error: {e}\n--- program ---\n{body}");
    }
}

#[track_caller]
fn bad(body: &str, fragment: &str) {
    let src = format!("{PRELUDE}\n{body}");
    match check(&src) {
        Ok(()) => panic!(
            "expected an error containing {fragment:?}, but it compiled\n--- program ---\n{body}"
        ),
        Err(e) if e.contains(fragment) => {}
        Err(e) => {
            panic!("expected an error containing {fragment:?}, got: {e}\n--- program ---\n{body}")
        }
    }
}

const PRELUDE: &str = r#"
struct Point { x: i64, y: i64 }
impl Point {
    fn len2(&self) -> i64 { return self.x * self.x + self.y * self.y }
    fn shift(&mut self, d: i64) { self.x += d }
}
"#;

#[test]
fn let_is_immutable_by_default() {
    bad("fn main() { let x = 1; x = 2 }", "immutable variable 'x'");
    ok("fn main() { let mut x = 1; x = 2; x += 3 }");
}

#[test]
fn mut_borrow_needs_mut_place() {
    bad(
        "fn main() { let x = 1; let r = &mut x }",
        "cannot borrow 'x' as mutable",
    );
    ok("fn main() { let mut x = 1; let r = &mut x; *r = 5 }");
}

#[test]
fn no_write_through_shared_ref() {
    bad(
        "fn main() { let mut x = 1; let r = &x; *r = 5 }",
        "cannot assign through",
    );
    bad(
        "fn f(p: &Point) { p.x = 1 }\nfn main() {}",
        "cannot assign through",
    );
    ok("fn f(p: &mut Point) { p.x = 1 }\nfn main() {}");
}

#[test]
fn methods_respect_receiver_mutability() {
    bad(
        "fn main() { let p = Point { x: 1, y: 2 }; p.shift(1) }",
        "needs '&mut self'",
    );
    bad(
        "fn f(p: &Point) { p.shift(1) }\nfn main() {}",
        "needs '&mut self'",
    );
    ok("fn main() { let mut p = Point { x: 1, y: 2 }; p.shift(1); let n = p.len2() }");
    ok("fn f(p: &mut Point) -> i64 { p.shift(1); return p.len2() }\nfn main() {}");
}

#[test]
fn mut_ref_coerces_to_shared() {
    ok(
        "fn read(p: &Point) -> i64 { return p.x }\nfn f(p: &mut Point) -> i64 { return read(p) }\nfn main() {}",
    );
    bad(
        "fn write(p: &mut Point) {}\nfn f(p: &Point) { write(p) }\nfn main() {}",
        "expected &mut Point, found &Point",
    );
}

#[test]
fn raw_deref_only_in_trusted_files() {
    bad(
        "fn main() { let p = 0xB8000 as *mut u16; *p = 1 }",
        "only allowed in a file that starts with #![trusted]",
    );
    bad(
        "fn main() { let p = 0 as *const u8; let v = p.read() }",
        "#![trusted]",
    );
    trusted_ok("fn main() { let p = 0xB8000 as *mut u16; *p = 0x0741; p.add(1).write(0x0742) }");
}

#[test]
fn raw_pointer_creation_is_allowed_everywhere() {
    ok(
        "fn main() { let mut x = 5; let p = &mut x as *mut i64; let q = p as *const i64; let a = q as usize; let n = p.is_null() }",
    );
}

#[test]
fn hardware_access_only_in_trusted_files() {
    bad("fn main() { outb(0x3F8, 65) }", "'outb' is only allowed");
    ok("fn main() { let c = rdtsc(); cli(); sti() }");
    trusted_ok("fn main() { outb(0x3F8, 65); let v = inb(0x60) }");
}

#[test]
fn trusted_file_exposes_a_safe_api() {
    let driver = "#![trusted]
        struct Port8 { port: u16 }
        impl Port8 {
            fn write(&self, v: u8) { outb(self.port, v) }
            fn read(&self) -> u8 { return inb(self.port) }
        }";
    let kernel =
        "fn main() { let com1 = Port8 { port: 0x3F8 }; com1.write(65); let v = com1.read() }";
    if let Err(e) = check_files(&[driver, kernel]) {
        panic!("expected success, got {e}");
    }
    let sneaky = "fn main() { outb(0x3F8, 65) }";
    assert!(
        check_files(&[driver, sneaky])
            .unwrap_err()
            .contains("only allowed")
    );
}

#[test]
fn static_mut_is_usable_everywhere() {
    ok("static mut TICKS: u64 = 0\nfn main() { TICKS += 1; let t = TICKS }");
    bad(
        "static LIMIT: u64 = 10\nfn main() { LIMIT = 3 }",
        "cannot assign",
    );
    ok("static LIMIT: u64 = 10\nfn main() { let l = LIMIT + 1 }");
}

#[test]
fn unsafe_keyword_does_not_exist() {
    bad("fn main() { unsafe { cli() } }", "'unsafe' does not exist");
    bad("unsafe fn f() {}\nfn main() {}", "'unsafe' does not exist");
}

#[test]
fn cannot_return_ref_to_local() {
    bad(
        "fn f() -> &i64 { let x = 1; return &x }\nfn main() {}",
        "reference to a local variable",
    );
    bad(
        "fn f() -> &i64 { let x = 1; let r = &x; return r }\nfn main() {}",
        "reference to a local variable",
    );
    bad(
        "fn f(v: i64) -> &i64 { return &v }\nfn main() {}",
        "reference to a local variable",
    );
    bad(
        "fn f() -> &[u8] { let a = [1u8, 2, 3]; return &a[0..2] }\nfn main() {}",
        "reference to a local variable",
    );
}

#[test]
fn can_return_ref_derived_from_param_or_static() {
    ok("fn first(p: &Point) -> &i64 { return &p.x }\nfn main() {}");
    ok("fn pick(a: &[u8]) -> &[u8] { return &a[1..] }\nfn main() {}");
    ok("static TABLE: [u8; 4] = [1, 2, 3, 4]\nfn t() -> &[u8] { return &TABLE }\nfn main() {}");
    ok("fn id(p: &mut Point) -> &mut Point { return p }\nfn main() {}");
}

#[test]
fn refs_cannot_be_stored_in_structs_or_statics() {
    bad(
        "struct Holder { r: &u8 }\nfn main() {}",
        "references cannot be stored",
    );
    bad(
        "static mut R: [&u8; 1]\nfn main() {}",
        "references cannot be stored",
    );
    bad(
        "fn main() { let x = 1; let r = &x; let rr = &r }",
        "references to references",
    );
}

#[test]
fn refs_are_never_null() {
    bad("fn main() { let r: &u8 }", "must be initialized");
    bad("fn main() { let r: &u8 = 0 }", "expected &u8, found i64");
}

#[test]
fn no_implicit_integer_conversion() {
    bad(
        "fn main() { let a: u8 = 1; let b: u16 = 2; let c = a + b }",
        "use 'as' to convert",
    );
    bad(
        "fn f(x: u64) {}\nfn main() { let n: usize = 3; f(n) }",
        "expected u64, found usize",
    );
    ok("fn f(x: u64) {}\nfn main() { let n: usize = 3; f(n as u64) }");
}

#[test]
fn literals_take_the_context_type() {
    ok("fn main() { let a: u8 = 255; let b = a + 1; let c: u64 = 0xFFFF_FFFF_FFFF_FFFF }");
    bad("fn main() { let a: u8 = 256 }", "out of range for u8");
    bad(
        "fn main() { let a: u8 = 3; let b = a + 300 }",
        "out of range for u8",
    );
    bad("fn main() { let a: u32 = -1 }", "out of range for u32");
}

#[test]
fn indexing_needs_usize_and_checks_constant_bounds() {
    bad(
        "fn main() { let a = [1, 2, 3]; let i: i64 = 0; let x = a[i] }",
        "expected usize, found i64",
    );
    bad(
        "fn main() { let a = [1, 2, 3]; let x = a[3] }",
        "out of bounds",
    );
    ok("fn main() { let a = [1, 2, 3]; let i: usize = 2; let x = a[i] + a[0] }");
}

#[test]
fn no_pointer_arithmetic_or_raw_indexing() {
    bad(
        "fn main() { let p = 0 as *const u8; let q = p + 1 }",
        "pointer arithmetic is not allowed",
    );
    bad(
        "fn main() { let p = 0 as *const u8; let q = p[1] }",
        "raw pointers cannot be indexed",
    );
}

#[test]
fn slices() {
    ok("fn sum(s: &[u8]) -> u64 { let mut t: u64 = 0; for i in 0..s.len() { t += s[i] as u64 }; return t }
        fn main() { let a: [u8; 4] = [1, 2, 3, 4]; let s = sum(&a); let t = sum(&a[1..3]) }");
    ok("fn fill(s: &mut [u8]) { for i in 0..s.len() { s[i] = 7 } }
        fn main() { let mut a = [0u8; 8]; fill(&mut a); fill(&mut a[2..]) }");
    bad(
        "fn fill(s: &mut [u8]) {}\nfn main() { let a = [0u8; 8]; fill(&mut a) }",
        "cannot borrow 'a' as mutable",
    );
    bad("fn f(s: &[u8]) { s[0] = 1 }\nfn main() {}", "cannot assign");
    bad(
        "fn main() { let a = [0; 8]; let s = &a[2..9] }",
        "out of bounds",
    );
    bad(
        "fn main() { let a = [0; 8]; let s = a[1..2] }",
        "slicing needs a reference",
    );
}

#[test]
fn casts_follow_rust_rules() {
    ok(
        "fn main() { let x: i64 = -1; let y = x as u8; let b = true as u8; let p = &x as *const i64 as usize }",
    );
    bad(
        "fn main() { let x: u64 = 1; let b = x as bool }",
        "cannot cast u64 as bool",
    );
    bad(
        "fn main() { let x = 5; let r = &x; let p = r as *mut i64 }",
        "cannot cast &i64 as *mut i64",
    );
}

#[test]
fn enums_and_match() {
    ok("enum Color: u8 { Red, Green = 5, Blue }
        fn name(c: Color) -> i64 { match c { Color::Red => return 1, Color::Green | Color::Blue => return 2 } }
        fn main() { let c = Color::Blue; let n = c as u8 }");
    bad(
        "enum Color: u8 { Red, Green }\nfn main() { let c: Color = 1 }",
        "expected Color, found",
    );
    bad(
        "fn main() { let x = 3; match x { 1 => {}, 1 => {} } }",
        "already covered",
    );
}

#[test]
fn integer_suffixes() {
    ok("fn main() { let a = 5u8; let b: u8 = a + 1; let c = [0u16; 4]; let d = -3i8 }");
    bad("fn main() { let a = 300u8 }", "out of range for u8");
    bad("fn main() { let a: u16 = 5u8 }", "expected u16, found u8");
}

#[test]
fn constants_and_array_sizes() {
    ok(
        "const N: usize = 4\nconst MASK = 0xFF\nstatic mut BUF: [u8; N * 2] = [0; N * 2]\n
        fn main() { let a: u8 = MASK; let b: u32 = MASK; BUF[7] = a }",
    );
    bad(
        "const BIG: u8 = 255 + 1\nfn main() {}",
        "out of range for u8",
    );
}

#[test]
fn struct_literals_must_be_complete() {
    bad(
        "fn main() { let p = Point { x: 1 } }",
        "missing field(s) in 'Point': y",
    );
    bad(
        "fn main() { let p = Point { x: 1, y: 2, z: 3 } }",
        "has no field 'z'",
    );
}

#[test]
fn efi_calls_only_in_trusted_files() {
    trusted_ok(
        "struct Out { output: extern fn(*mut Out, *const u16) -> u64 }
        fn main() { let o = efi_system_table() as *mut Out; let f = (*o).output; f(o, u\"hi\") }",
    );
    bad(
        "struct Out { output: extern fn(*mut Out, *const u16) -> u64 }
        fn say(o: &Out, p: *mut Out) { o.output(p, u\"hi\") }\nfn main() {}",
        "firmware code) is only allowed",
    );
}

#[test]
fn interrupt_handlers() {
    ok(
        "struct Frame { rip: u64, cs: u64, flags: u64, rsp: u64, ss: u64 }
        #[interrupt] fn tick(frame: &Frame) {}
        #[interrupt] fn fault(frame: &Frame, code: u64) {}
        fn main() { let a = tick as u64; let b = fault as usize }",
    );
    bad(
        "struct Frame { rip: u64 }\n#[interrupt] fn tick(frame: &Frame) {}\nfn main() { tick(0 as *const Frame) }",
        "cannot be called directly",
    );
}
