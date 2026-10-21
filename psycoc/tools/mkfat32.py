import os
import struct
import sys
import zlib

sector = 512
reserved = 32
fats = 2
fat_date = ((2026 - 1980) << 9) | (1 << 5) | 1
valid = set("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&'()-@^_`{}~")


class Fat32:
    def __init__(self, sectors, label, hidden) -> None:
        self.sectors = sectors
        self.hidden = hidden
        self.spc = 1 if sectors * sector <= 260 << 20 else 8
        fat = 1
        while True:
            clusters = (sectors - reserved - fats * fat) // self.spc
            need = ((clusters + 2) * 4 + sector - 1) // sector
            if need <= fat:
                break
            fat = need
        self.fat_sectors = fat
        self.clusters = clusters
        if clusters < 65525:
            sys.exit("mkfat32: image too small for FAT32 (use --size 40 or more)")
        self.cluster_bytes = self.spc * sector
        self.data_start = reserved + fats * fat
        self.fat = [0] * (clusters + 2)
        self.fat[0] = 0x0FFFFFF8
        self.fat[1] = 0x0FFFFFFF
        self.next_free = 2
        self.data = {}
        self.label = label

    def alloc(self, count):
        count = max(count, 1)
        first = self.next_free
        if first + count > self.clusters + 2:
            sys.exit("mkfat32: image full")
        for i in range(count):
            c = first + i
            self.fat[c] = c + 1 if i + 1 < count else 0x0FFFFFFF
        self.next_free += count
        return first

    def write_chain(self, first, payload):
        for i in range(0, max(len(payload), 1), self.cluster_bytes):
            chunk = payload[i : i + self.cluster_bytes]
            self.data[first + i // self.cluster_bytes] = chunk.ljust(
                self.cluster_bytes, b"\n"
            )

    def image(self):
        out = bytearray(self.sectors * sector)
        b = bytearray(sector)
        b[0:3] = b"\xeb\x58\x90"
        b[3:11] = b"PSYCOS  "
        struct.pack_into(
            "<HBHBHHBHHHII",
            b,
            11,
            sector,
            self.spc,
            reserved,
            fats,
            0,
            0,
            0xF8,
            0,
            32,
            64,
            self.hidden,
            self.sectors,
        )
        struct.pack_into("<IHHIHH", b, 36, self.fat_sectors, 0, 0, 2, 1, 6)
        struct.pack_into("<BBBI", b, 64, 0x80, 0, 0x29, 0x50535943)
        b[71:82] = self.label.encode().ljust(11)[:11]
        b[82:90] = b"Fat32   "
        b[510:512] = b"\x55\xaa"
        info = bytearray(sector)
        free = self.clusters + 2 - self.next_free
        struct.pack_into("<I", info, 0, 0x41615252)
        struct.pack_into("<III", info, 484, 0x61417272, free, self.next_free)
        struct.pack_into("<I", info, 508, 0xAA550000)
        for base in (0, 6):
            out[(base) * sector : (base + 1) * sector] = b
            out[(base + 1) * sector : (base + 2) * sector] = info
        fat = struct.pack("<%dI" % len(self.fat), *self.fat)
        for i in range(fats):
            start = (reserved + i * self.fat_sectors) * sector
            out[start : start + len(fat)] = fat
        for cluster, chunk in self.data.items():
            start = (self.data_start + (cluster - 2) * self.spc) * sector
            out[start : start + len(chunk)] = chunk
        return out


def short_name(name, taken):
    base, dot, ext = name.rpartition(".")
    if not dot:
        base, ext = name, ""
    simple = (
        0 < len(base) <= 8
        and len(ext) <= 3
        and name.count(".") <= 1
        and all(c.upper() in valid for c in base + ext)
    )
    if simple:
        flags = 0
        for part, flag in ((base, 0x08), (ext, 0x10)):
            if part.islower():
                flags |= flag
            elif part != part.upper():
                simple = False
        if simple:
            short = (base.upper().ljust(8) + ext.upper().ljust(3)).encode()
            if short not in taken:
                return short, flags, False
    clean = "".join(c for c in base.upper() if c in valid) or "FILE"
    ext_clean = "".join(c for c in ext.upper() if c in valid)[:3]
    for n in range(1, 1000):
        tail = "~%d" % n
        short = (clean[: 8 - len(tail)] + tail).ljust(8) + ext_clean.ljust(3)
        if short.encode() not in taken:
            return short.encode(), 0, True
    sys.exit("mkfat32: too many similar names")


def entry(short, attr, cluster, size, ntres=0):
    return struct.pack(
        "<11sBBBHHHHHHHI",
        short,
        attr,
        ntres,
        0,
        12 << 11,
        fat_date,
        fat_date,
        cluster >> 16,
        12 << 11,
        fat_date,
        cluster & 0xFFFF,
        size,
    )


def checksum(short):
    s = 0
    for c in short:
        s = (((s & 1) << 7) + (s >> 1) + c) & 0xFF
    return s


def lfn_entries(name, short):
    units = list(name.encode("utf-16-le"))
    chars = [units[i] | (units[i + 1] << 8) for i in range(0, len(units), 2)]
    chars.append(0)
    while len(chars) % 13:
        chars.append(0xFFFF)
    parts = [chars[i : i + 13] for i in range(0, len(chars), 13)]
    out = []
    for seq in range(len(parts), 0, -1):
        c = parts[seq - 1]
        order = seq | (0x40 if seq == len(parts) else 0)
        e = struct.pack(
            "<B5HBBB6HH2H",
            order,
            *c[0:5],
            0x0F,
            0,
            checksum(short),
            *c[5:11],
            0,
            *c[11:13],
        )
        out.append(e)
    return out


def add_dir(fs, path, cluster, parent):
    entries = []
    if parent is None:
        entries.append(entry(fs.label.encode().ljust(11)[:11], 0x08, 0, 0))
    else:
        entries.append(entry(b".          ", 0x10, cluster, 0))
        entries.append(entry(b"..         ", 0x10, parent, 0))
    taken = set()
    for name in sorted(os.listdir(path)):
        full = os.path.join(path, name)
        short, ntres, long = short_name(name, taken)
        taken.add(short)
        if long:
            entries.extend(lfn_entries(name, short))

        if os.path.isdir(full):
            sub = fs.alloc(1)
            entries.append(entry(short, 0x10, sub, 0, ntres))
            add_dir(fs, full, sub, cluster if parent is not None else 0)
        else:
            with open(full, "rb") as f:
                data = f.read()

            first = (
                fs.alloc((len(data) + fs.cluster_bytes - 1) // fs.cluster_bytes)
                if data
                else 0
            )

            if data:
                fs.write_chain(first, data)

            entries.append(entry(short, 0x20, first, len(data), ntres))

    payload = b"".join(entries)
    need = max(1, (len(payload) + 32 + fs.cluster_bytes - 1) // fs.cluster_bytes)

    chain = [cluster]
    if need > 1:
        extra = fs.alloc(need - 1)
        fs.fat[cluster] = extra
        chain += list(range(extra, extra + need - 1))

    for i, c in enumerate(chain):
        chunk = payload[i * fs.cluster_bytes : (i + 1) * fs.cluster_bytes]
        fs.data[c] = chunk.ljust(fs.cluster_bytes, b"\0")


esp_type = bytes([0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B])


def gpt_header(total, current, backup, entries_lba, entries_crc):
    h = bytearray(sector)
    struct.pack_into(
        "<8sIIIIQQQQ16sQIII",
        h,
        0,
        b"EFI PART",
        0x00010000,
        92,
        0,
        0,
        current,
        backup,
        34,
        total - 34,
        b"psyos-usb-disk!!",
        entries_lba,
        128,
        128,
        entries_crc,
    )
    struct.pack_into("<I", h, 16, zlib.crc32(bytes(h[:92])))
    return h


def write_gpt(disk, total, first, last):
    disk[446:462] = struct.pack("<B3sB3sII", 0, b"\x00\x02\x00", 0xEE, b"\xff\xff\xff", 1, min(total - 1, 0xFFFFFFFF))
    disk[510:512] = b"\x55\xaa"
    entries = bytearray(128 * 128)
    name = "PSYOS".encode("utf-16-le")
    struct.pack_into("<16s16sQQQ", entries, 0, esp_type, b"psyos-partition1", first, last, 0)
    entries[56 : 56 + len(name)] = name
    crc = zlib.crc32(bytes(entries))
    disk[2 * sector : 2 * sector + len(entries)] = entries
    backup_entries = total - 1 - len(entries) // sector
    disk[backup_entries * sector : backup_entries * sector + len(entries)] = entries
    disk[sector : 2 * sector] = gpt_header(total, 1, total - 1, 2, crc)
    disk[(total - 1) * sector : total * sector] = gpt_header(total, total - 1, 1, backup_entries, crc)


def main():
    args = sys.argv[1:]
    size, mbr, gpt, label = 64, False, False, "PSYCOS"
    rest = []
    i = 0
    while i < len(args):
        if args[i] == "--size":
            size = int(args[i + 1])
            i += 2
        elif args[i] == "--label":
            label = args[i + 1].upper()
            i += 2
        elif args[i] == "--mbr":
            mbr = True
            i += 1
        elif args[i] == "--gpt":
            gpt = True
            i += 1
        else:
            rest.append(args[i])
            i += 1
    if len(rest) != 2:
        sys.exit(__doc__)
    image_path, src = rest
    total = (size << 20) // sector
    start = 2048 if mbr or gpt else 0
    end = total - 2048 if gpt else total
    fs = Fat32(end - start, label, start)
    root = fs.alloc(1)
    add_dir(fs, src, root, None)
    image = fs.image()
    if mbr:
        disk = bytearray(total * sector)
        disk[start * sector :] = image
        part = struct.pack(
            "<B3sB3sII", 0, b"\x00\x02\x00", 0x0C, b"\xfe\xff\xff", start, total - start
        )
        disk[446:462] = part
        disk[510:512] = b"\x55\xaa"
        image = disk
    elif gpt:
        disk = bytearray(total * sector)
        disk[start * sector : end * sector] = image
        write_gpt(disk, total, start, end - 1)
        image = disk
    with open(image_path, "wb") as f:
        f.write(image)


if __name__ == "__main__":
    main()
