use psycoc::{lexer::Lexer, parser::Parser, typeck::TypeChecker};

fn check(src: &str) -> Result<(), String> {
    let tokens = Lexer::new(src)
        .tokenize()
        .map_err(|e| format!("lexer: {}", e.message))?;
    let mut program = Parser::new(tokens)
        .parse_program()
        .map_err(|e| format!("parser {}:{}: {}", e.span.line, e.span.col, e.message))?;
    program.merge(psycoc::driver::prelude(1)?);
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
fn memory_primitives_only_in_trusted_files() {
    bad(
        "fn main() { let mut a = [0u32; 4]; memset32(a.as_mut_ptr(), 1, 4) }",
        "'memset32' is only allowed",
    );
    bad("fn main() { let mut a = [0u8; 4]; memset(a.as_mut_ptr(), 1, 4) }", "'memset' is only allowed");
    trusted_ok("fn main() { let mut a = [0u32; 4]; memset32(a.as_mut_ptr(), 1, 4) }");
}

#[test]
fn atomic_swap_only_in_trusted_files() {
    bad(
        "static mut lock: u64 = 0\nfn main() { let old = atomic_swap(&mut lock as *mut u64, 1) }",
        "'atomic_swap' is only allowed",
    );
    trusted_ok("static mut lock: u64 = 0\nfn main() { let old = atomic_swap(&mut lock as *mut u64, 1) }");
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

#[test]
fn array_literal_takes_the_slice_element_type() {
    ok("fn f(b: &[u8]) {}\nfn main() { f(&[0x23, 255]); f(&[0; 4]) }");
    ok("fn f(b: &mut [i8]) {}\nfn main() { f(&mut [-128, 127]) }");
    bad(
        "fn f(b: &[u8]) {}\nfn main() { f(&[0x23, 256]) }",
        "literal 256 out of range for u8",
    );
    bad(
        "fn f() -> &[u8] { return &[1, 2] }\nfn main() {}",
        "cannot return a reference to a local",
    );
}

#[test]
fn str_as_bytes_is_a_shared_slice() {
    ok("fn f() -> &[u8] { return \"ab\".as_bytes() }\nfn main() {}");
    bad(
        "fn f(b: &mut [u8]) {}\nfn main() { f(\"ab\".as_bytes()) }",
        "expected &mut [u8], found &[u8]",
    );
    bad(
        "fn main() { let b = \"ab\".as_bytes(); b[0] = 1 }",
        "cannot assign",
    );
    bad("fn main() { let b = \"ab\".as_bytes(1) }", "expects 0 argument(s)");
}

#[test]
fn array_constants_are_read_only() {
    ok("const t: [u8; 3] = [1, 2, 3]\nfn f() -> &[u8] { return &t }\nfn main() { let x = t[1] }");
    bad(
        "const t: [u8; 2] = [1, 2]\nfn main() { t[0] = 5 }",
        "cannot assign to constant 't'",
    );
    bad(
        "const t: [u8; 2] = [1, 2]\nfn main() { let r = &mut t }",
        "cannot borrow constant 't' as mutable",
    );
    bad(
        "static s: [u8; 2] = [1, 2]\nfn main() { s[1] += 1 }",
        "cannot assign to immutable static 's'",
    );
    bad("const t = [1u8, 2]\nfn main() {}", "array constant 't' needs a type");
    bad("const t: [u8; 3] = [1, 2]\nfn main() {}", "expected [u8; 3], found [u8; 2]");
    bad(
        "const t: [u8; 2] = [1, 2]\nstatic t: u8 = 1\nfn main() {}",
        "'t' is already defined",
    );
}

const EVENT: &str = "enum Event { Key(u8), Tick, Move(i32, i32) }\nenum Shape { Dot, Line(Event, u64) }\n";

#[test]
fn match_must_be_exhaustive() {
    ok(&format!("{EVENT}fn f(e: Event) -> u8 {{ match e {{ Event::Key(c) => return c, Event::Tick => return 0, Event::Move(..) => return 1 }} }}\nfn main() {{}}").replace("Move(..)", "Move(_, _)"));
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Key(_) => {{}}, Event::Tick => {{}} }} }}"),
        "Event::Move(_, _) is missing",
    );
    bad(
        &format!("{EVENT}fn main() {{ let s = Shape::Dot; match s {{ Shape::Dot => {{}}, Shape::Line(Event::Tick, _) => {{}}, Shape::Line(Event::Move(_, _), _) => {{}} }} }}"),
        "Shape::Line(Event::Key(_), _) is missing",
    );
    bad(
        "fn main() { let x = 3; match x { 1 => {}, 2 => {} } }",
        "_ is missing",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Key(c) if c > 3 => {{}}, Event::Tick | Event::Move(_, _) => {{}} }} }}"),
        "Event::Key(_) is missing",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ _ => {{}}, Event::Tick => {{}} }} }}"),
        "already covered",
    );
}

#[test]
fn enum_patterns_are_checked() {
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Key(a) | Event::Move(a, _) => {{}}, _ => {{}} }} }}"),
        "cannot bind names",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Move(x) => {{}}, _ => {{}} }} }}"),
        "has 2 value(s), the pattern gives 1",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Key => {{}}, _ => {{}} }} }}"),
        "has data",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Jump(_) => {{}}, _ => {{}} }} }}"),
        "has no variant 'Jump'",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Tick; match e {{ Event::Key(x, x) => {{}}, _ => {{}} }} }}"),
        "has 1 value(s)",
    );
    bad(
        &format!("{EVENT}fn main() {{ let e = Event::Move(1, 2); match e {{ Event::Move(x, x) => {{}}, _ => {{}} }} }}"),
        "'x' is bound twice",
    );
    bad(
        &format!("{EVENT}fn f(e: &Event) {{ match e {{ Event::Key(k) => *k = 1, _ => {{}} }} }}\nfn main() {{}}"),
        "cannot assign",
    );
    ok(&format!("{EVENT}fn f(e: &mut Event) {{ match e {{ Event::Key(k) => *k = 1, _ => {{}} }} }}\nfn main() {{}}"));
    bad("struct P { x: u8 }\nfn main() { let p = P { x: 1 }; match p { _ => {} } }", "cannot match on a value of type P");
}

#[test]
fn enum_values_are_checked() {
    bad(&format!("{EVENT}fn main() {{ let e = Event::Key(300) }}"), "literal 300 out of range for u8");
    bad(&format!("{EVENT}fn main() {{ let e = Event::Move(1) }}"), "expects 2 value(s), got 1");
    bad(&format!("{EVENT}fn main() {{ let e = Event::Tick(1) }}"), "has no data");
    bad(&format!("{EVENT}fn main() {{ let a = Event::Tick; let b = Event::Tick; let c = a == b }}"), "use 'match' or 'if let'");
    bad("enum E { A(u8), B = 3 }\nfn main() {}", "cannot set discriminants");
    bad("enum L { End, Cons(u8, L) }\nfn main() {}", "contains itself");
}

#[test]
fn enums_holding_references() {
    let opt = "enum Opt { No, Yes(&u64) }\n";
    ok(&format!("{opt}fn f(r: &u64) -> Opt {{ return Opt::Yes(r) }}\nfn main() {{ let v: u64 = 1; let o = f(&v) }}"));
    bad(
        &format!("{opt}fn f() -> Opt {{ let v: u64 = 1; return Opt::Yes(&v) }}\nfn main() {{}}"),
        "cannot return a reference to a local",
    );
    bad(
        &format!("{opt}fn f() -> Opt {{ let v: u64 = 1; let o = Opt::Yes(&v); return o }}\nfn main() {{}}"),
        "cannot return a reference to a local",
    );
    bad(&format!("{opt}struct S {{ o: Opt }}\nfn main() {{}}"), "'Opt' holds a reference");
    bad(&format!("{opt}static s: Opt = Opt::No\nfn main() {{}}"), "'Opt' holds a reference");
    bad(&format!("{opt}fn main() {{ let a = [Opt::No; 2] }}"), "'Opt' holds a reference");
    bad(&format!("{opt}enum Two {{ A(Opt), B }}\nfn main() {{}}"), "cannot be the payload of another enum");
}

#[test]
fn generics_are_checked_per_instance() {
    let max = "fn max<T>(a: T, b: T) -> T { if a > b { return a } return b }\n";
    ok(&format!("{max}fn main() {{ let a = max(1u8, 2); let b = max::<i32>(3, 4) }}"));
    bad(&format!("{max}fn main() {{ let a = max(1u8, 2u16) }}"), "expected u8, found u16");
    bad(&format!("{max}fn main() {{ let a = max::<u8, u8>(1, 2) }}"), "takes 1 type argument(s), got 2");
    bad(&format!("{max}fn main() {{ let a = max(true, false) }}"), "cannot apply '>' to bool and bool");
    bad(&format!("{max}fn main() {{ let f = max }}"), "'max' is generic");
    bad("fn none<T>() -> u8 { return 0 }\nfn main() { let x = none() }", "cannot infer the type parameter 'T'");
    bad("struct Port<T> { v: T }\nfn main() { let p: Port = Port { v: 1 } }", "'Port' is generic");
    bad("struct Port<T> { v: T }\nfn f(r: &u8) { let p = Port { v: r } }\nfn main() {}", "references cannot be stored");
    bad("struct W<T> { v: T }\nfn deep<T>(x: T) { deep(W { v: x }) }\nfn main() { deep(1) }", "too many generic instantiations");
    bad("struct Port<T> { v: T }\nfn main() { let p = Port::<u8, u8> { v: 1 } }", "takes 1 type argument(s), got 2");
    ok("fn main() { let a: u8 = 1; let b = a as u64 < 3 }");
}

#[test]
fn impl_templates() {
    let cell = "struct Cell<T> { v: T }\nimpl<T> Cell<T> { fn get(&self) -> T { return self.v } fn new(v: T) -> Cell<T> { return Cell { v: v } } }\n";
    ok(&format!("{cell}fn main() {{ let c = Cell::new(3u8); let d: Cell<u16> = Cell::new(4); let e = Cell::<i8>::new(-1); let x = c.get() }}"));
    bad(&format!("{cell}fn main() {{ let c = Cell::new(3u8); c.set(1) }}"), "no method 'set'");
    bad("struct Cell<T> { v: T }\nimpl Cell { fn f(&self) {} }\nfn main() {}", "'Cell' is generic");
    ok("struct P<T> { v: T }\nimpl P<u8> { fn only_u8(&self) -> u8 { return self.v } }\nfn main() { let p = P { v: 1u8 }; let x = p.only_u8() }");
    bad("struct P<T> { v: T }\nimpl P<u8> { fn only_u8(&self) -> u8 { return self.v } }\nfn main() { let p = P { v: 1u16 }; let x = p.only_u8() }", "no method 'only_u8'");
}

#[test]
fn option_result_and_try() {
    ok("fn f(x: u8) -> Option<u8> { if x > 3 { return Some(x) } return None }\nfn main() { let a = f(4).unwrap_or(0) }");
    bad("fn main() { let x = None }", "cannot infer the type parameter 'T' of 'Option::None'");
    ok("fn main() { let x: Option<u16> = None; let y = Some(3u8) }");
    bad("fn main() { let x: Option<u16> = Some(true) }", "expected u16, found bool");
    bad(
        "fn g() -> Option<u8> { return Some(1) }\nfn f() -> u8 { let v = g()?; return v }\nfn main() {}",
        "the function must return an Option",
    );
    bad(
        "fn g() -> Result<u8, u16> { return Ok(1) }\nfn f() -> Result<u8, u32> { let v = g()?; return Ok(v) }\nfn main() {}",
        "cannot pass a u16 error to a function whose error type is u32",
    );
    bad(
        "fn g() -> Option<u8> { return Some(1) }\nfn f() -> Result<u8, u8> { let v = g()?; return Ok(v) }\nfn main() {}",
        "must return an Option",
    );
    bad("fn f() -> Option<u8> { let x = 5u8?; return None }\nfn main() {}", "'?' needs an Option or a Result");
    bad(
        "fn main() { let o: Option<u8> = None; match o { Some(x) => {} } }",
        "Option<u8>::None is missing",
    );
    bad(
        "fn f() -> Option<&u64> { let v: u64 = 3; return Some(&v) }\nfn main() {}",
        "cannot return a reference to a local",
    );
    ok("fn Some(x: u8) -> u8 { return x }\nfn main() { let y: u8 = Some(3) }");
}

const SHAPE: &str = "trait Shape { fn area(&self) -> u64\n fn sides(&self) -> u8 { return 0 } }\nstruct Sq { s: u64 }\n";

#[test]
fn trait_impls_are_checked() {
    ok(&format!("{SHAPE}impl Shape for Sq {{ fn area(&self) -> u64 {{ return self.s }} }}\nfn main() {{}}"));
    bad(&format!("{SHAPE}impl Shape for Sq {{ fn sides(&self) -> u8 {{ return 4 }} }}\nfn main() {{}}"), "missing method 'area'");
    bad(
        &format!("{SHAPE}impl Shape for Sq {{ fn area(&self) -> u64 {{ return 1 }} fn volume(&self) -> u64 {{ return 1 }} }}\nfn main() {{}}"),
        "'volume' is not a method of trait 'Shape'",
    );
    bad(
        &format!("{SHAPE}impl Shape for Sq {{ fn area(&self) -> u32 {{ return 1 }} }}\nfn main() {{}}"),
        "does not match trait 'Shape'",
    );
    bad(
        &format!("{SHAPE}impl Shape for Sq {{ fn area(&self) -> u64 {{ return true }} }}\nfn main() {{}}"),
        "expected u64, found bool",
    );
    bad(
        &format!("{SHAPE}impl Shape for Sq {{ fn area(&self) -> u64 {{ return 1 }} }}\nimpl Shape for Sq {{ fn area(&self) -> u64 {{ return 2 }} }}\nfn main() {{}}"),
        "implemented twice",
    );
    bad("impl Draw for u8 { fn d(&self) {} }\nfn main() {}", "unknown trait 'Draw'");
}

#[test]
fn trait_bounds_are_checked() {
    let base = format!("{SHAPE}impl Shape for Sq {{ fn area(&self) -> u64 {{ return self.s }} }}\nfn total<T: Shape>(x: &T) -> u64 {{ return x.area() }}\n");
    ok(&format!("{base}fn main() {{ let a = total(&Sq {{ s: 1 }}) }}"));
    bad(&format!("{base}fn main() {{ let a = total(&5u8) }}"), "u8 does not implement 'Shape'");
    bad(&format!("{base}fn f<T: Missing>(x: T) {{}}\nfn main() {{ f(1) }}"), "unknown trait 'Missing'");
    bad(
        "trait A { fn go(&self) -> u8 }\ntrait B { fn go(&self) -> u8 }\nstruct S { v: u8 }\nimpl A for S { fn go(&self) -> u8 { return 1 } }\nimpl B for S { fn go(&self) -> u8 { return 2 } }\nfn main() { let s = S { v: 0 }; let x = s.go() }",
        "is ambiguous",
    );
    ok("trait A { fn go(&self) -> u8 }\nstruct S { v: u8 }\nimpl S { fn go(&self) -> u8 { return 0 } }\nimpl A for S { fn go(&self) -> u8 { return 1 } }\nfn main() { let s = S { v: 0 }; let x = s.go() }");
}

#[test]
fn statics_with_addresses() {
    ok("fn a() {}\nstatic t: [fn(); 2] = [a, a]\nstatic s: Str = \"hi\"\nfn main() { t[0]() }");
    ok("static n: u64 = 1\nstatic p: *const u64 = &n as *const u64\nfn main() {}");
    bad("fn f() -> u64 { return 1 }\nstatic x: u64 = f()\nfn main() {}", "constant expressions");
    bad("fn main() { rebase_statics(0) }", "'rebase_statics' is only allowed");
    bad("static n: u64 = 1\nstatic p: *const u64 = &n as *const u64 + 1\nfn main() {}", "pointer arithmetic");
}

#[test]
fn modules_resolve_names() {
    ok("mod m { fn x() -> u8 { return 1 } fn f() -> u8 { let x = 5u8; return x } }\nfn main() { let a = m::f() }");
    ok("const k: u8 = 3\nmod m { fn f() -> u8 { return k } }\nfn main() { let a = m::f() }");
    bad("mod m { fn f() {} }\nfn main() { m::g() }", "m::g");
    bad("mod m { fn f() {} }\nfn main() { f() }", "unknown variable 'f'");
    bad("mod P { fn f() {} }\nstruct P { x: u8 }\nfn main() {}", "both a module and a type");
    ok("mod a { mod b { fn deep() -> u8 { return 1 } } }\nfn main() { let x = a::b::deep() }");
    bad("mod m { struct S { v: u8 } }\nfn main() { let s = S { v: 1 } }", "unknown struct 'S'");
}

#[test]
fn borrows_are_exclusive() {
    bad("fn main() { let mut x = 1; let a = &mut x; let b = &mut x; *a = 2; *b = 3 }", "cannot borrow 'x' as mutable while it is borrowed");
    bad("fn main() { let mut x = 1; let a = &mut x; let b = &x; *a = 2; let c = *b }", "cannot borrow 'x' while it is borrowed");
    bad("fn main() { let mut x = 1; let b = &x; let a = &mut x; *a = 2; let c = *b }", "cannot borrow 'x' as mutable");
    bad("fn main() { let mut x = 1; let a = &mut x; let y = x; *a = 2 }", "cannot use 'x' while it is borrowed");
    bad("fn main() { let mut x = 1; let b = &x; x = 5; let c = *b }", "cannot assign to 'x' while it is borrowed");
    bad("fn f(a: &mut u64, b: &u64) {}\nfn main() { let mut x: u64 = 1; f(&mut x, &x) }", "cannot borrow 'x' while it is borrowed");
    bad("fn f(p: &mut [u8], q: &mut [u8]) {}\nfn main() { let mut a = [1u8]; f(&mut a, &mut a) }", "cannot borrow 'a' as mutable");
    bad("fn main() { let mut x = 1; let a = &mut x; let b = a; *a = 1; *b = 2 }", "cannot assign to '*a'");
    bad("fn main() { let mut v = [1u8, 2, 3]; let s = &mut v[0..2]; v[2] = 9; s[0] = 4 }", "cannot assign to 'v[_]'");
    bad("struct S { v: [u8; 4] }\nfn g(s: &mut S) { let r = &mut s.v[1]; s.v[0] = 1; *r = 5 }\nfn main() {}", "cannot assign to '(*s).v[_]'");
    bad("fn main() { let mut v = [0u64; 2]; let r = &mut v; let mut i = 0; while i < 2 { v[0] = 1; r[1] = 2; i += 1 } }", "cannot assign to 'v[_]'");
    bad("fn pick(a: &mut [u64; 2]) -> &mut u64 { return &mut a[1] }\nfn main() { let mut v = [1u64, 2]; let r = pick(&mut v); v[0] = 3; *r = 9 }", "cannot assign to 'v[_]'");
}

#[test]
fn borrows_do_not_outlive_their_variable() {
    bad("fn main() { let a: u64 = 1; let mut r = &a; { let b: u64 = 2; r = &b } let c = *r }", "'b' does not live long enough");
    ok("fn main() { let a: u64 = 1; let mut r = &a; { let b: u64 = 2; r = &b; let c = *r } }");
}

#[test]
fn borrows_that_are_fine() {
    ok("struct P { a: u8, b: u8 }\nfn main() { let mut p = P { a: 1, b: 2 }; let r = &mut p.a; let s = &mut p.b; *r = 3; *s = 4 }");
    ok("fn main() { let mut x = 1; let a = &mut x; *a = 2; let y = x }");
    ok("struct V { n: u64 }\nimpl V { fn len(&self) -> u64 { return self.n } fn add(&mut self, k: u64) { self.n += k } }\nfn main() { let mut v = V { n: 3 }; v.add(v.len()) }");
    ok("fn inc(r: &mut u64) { *r += 1 }\nfn twice(r: &mut u64) { inc(r); inc(r) }\nfn main() { let mut x: u64 = 0; twice(&mut x) }");
    ok("fn main() { let mut v = [0u64; 4]; for i in 0..4usize { let r = &mut v[i]; *r = i as u64 } let k = v[3] }");
    ok("fn main() { let v = [1u64, 2, 3]; let mut best = &v[0]; for i in 0..3usize { if v[i] > *best { best = &v[i] } } let k = *best }");
    ok("fn main() { let mut x = 1; let r = &mut x; *r = 5; let s = &mut x; *s += 1 }");
}

#[test]
fn let_else_must_leave() {
    ok("fn f(o: Option<u64>) -> u64 { let Some(x) = o else { return 0 }; return x }\nfn main() { f(Some(1)) }");
    bad(
        "fn f(o: Option<u64>) -> u64 { let Some(x) = o else { print(1) }; return x }\nfn main() { f(Some(1)) }",
        "must leave",
    );
    bad(
        "fn f(o: Option<u64>) -> u64 { let Some(x) = o else { return x }; return x }\nfn main() { f(Some(1)) }",
        "unknown variable 'x'",
    );
}
