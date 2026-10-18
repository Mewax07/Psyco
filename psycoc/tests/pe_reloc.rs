use psycoc::{Target, compile_str};

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn load(pe: &[u8], base: u64) -> (Vec<u8>, usize) {
    let pe_off = u32_at(pe, 0x3C) as usize;
    let nsec = u16_at(pe, pe_off + 6) as usize;
    let opt = pe_off + 24;
    let image_base = u64_at(pe, opt + 24);
    let size_of_image = u32_at(pe, opt + 56) as usize;
    let reloc_rva = u32_at(pe, opt + 112 + 5 * 8) as usize;
    let reloc_size = u32_at(pe, opt + 112 + 5 * 8 + 4) as usize;
    let mut image = vec![0u8; size_of_image];
    let sec = opt + 240;
    for i in 0..nsec {
        let s = sec + 40 * i;
        let rva = u32_at(pe, s + 12) as usize;
        let raw_size = u32_at(pe, s + 16) as usize;
        let raw_off = u32_at(pe, s + 20) as usize;
        let n = raw_size.min(image.len() - rva);
        image[rva..rva + n].copy_from_slice(&pe[raw_off..raw_off + n]);
    }
    let delta = base.wrapping_sub(image_base);
    let mut count = 0;
    let mut p = reloc_rva;
    while p < reloc_rva + reloc_size {
        let page = u32_at(&image, p) as usize;
        let block = u32_at(&image, p + 4) as usize;
        assert!(block >= 8 && block % 4 == 0, "bad relocation block size {block}");
        for e in (p + 8..p + block).step_by(2) {
            let entry = u16_at(&image, e);
            match entry >> 12 {
                0 => {}
                10 => {
                    let at = page + (entry & 0xFFF) as usize;
                    let v = u64_at(&image, at);
                    assert!(
                        v >= image_base && v < image_base + size_of_image as u64,
                        "relocated slot at {at:#x} holds {v:#x}, outside the image"
                    );
                    image[at..at + 8].copy_from_slice(&v.wrapping_add(delta).to_le_bytes());
                    count += 1;
                }
                t => panic!("unexpected relocation type {t}"),
            }
        }
        p += block;
    }
    (image, count)
}

#[test]
fn statics_with_addresses_get_dir64_relocations() {
    let src = "fn a() {}\nfn b() {}\nstatic t: [fn(); 2] = [a, b]\nstatic s: [Str; 3] = [\"x\", \"y\", \"x\"]\nstatic n: u64 = 5\nstatic p: *const u64 = &n as *const u64\nfn main() { t[0](); print(s[1]) }";
    let pe = compile_str(src, Target::Uefi).unwrap();
    let (at_pref, n1) = load(&pe, 0x40_0000);
    assert_eq!(n1, 2 + 3 + 1);
    let base = 0x1234_5000u64;
    let (moved, n2) = load(&pe, base);
    assert_eq!(n2, n1);
    let delta = base - 0x40_0000;
    let diffs = at_pref
        .chunks(8)
        .zip(moved.chunks(8))
        .filter(|(x, y)| x != y)
        .map(|(x, y)| u64::from_le_bytes(y.try_into().unwrap()) - u64::from_le_bytes(x.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(diffs.len(), 6);
    assert!(diffs.iter().all(|&d| d == delta));
}

#[test]
fn image_without_addresses_has_an_empty_relocation_block() {
    let pe = compile_str("fn main() { print(1) }", Target::Uefi).unwrap();
    let (_, n) = load(&pe, 0x1000_0000);
    assert_eq!(n, 0);
}
