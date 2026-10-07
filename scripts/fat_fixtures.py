"""Add malformed FAT and ELF files to a private QEMU smoke disk copy."""

from __future__ import annotations

import struct
import subprocess
from pathlib import Path

SECTOR_SIZE = 512


def _u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


class FatImage:
    def __init__(self, path: Path):
        self.file = path.open("r+b")
        self.disk_size = path.stat().st_size
        boot = self.read(0, SECTOR_SIZE)
        self.sectors_per_cluster = boot[13]
        self.reserved = _u16(boot, 14)
        self.fat_count = boot[16]
        self.fat_sectors = _u32(boot, 36)
        self.root_cluster = _u32(boot, 44)
        total_sectors = _u16(boot, 19) or _u32(boot, 32)
        self.data_sector = self.reserved + self.fat_count * self.fat_sectors
        if (
            _u16(boot, 11) != SECTOR_SIZE
            or self.sectors_per_cluster == 0
            or self.sectors_per_cluster & (self.sectors_per_cluster - 1)
            or self.fat_count not in (1, 2)
            or self.fat_sectors == 0
            or self.root_cluster < 2
            or self.data_sector >= total_sectors
            or total_sectors * SECTOR_SIZE != self.disk_size
        ):
            raise ValueError("unexpected smoke FAT32 geometry")
        self.cluster_bytes = self.sectors_per_cluster * SECTOR_SIZE

    def close(self) -> None:
        self.file.close()

    def read(self, offset: int, size: int) -> bytes:
        self.file.seek(offset)
        data = self.file.read(size)
        if len(data) != size:
            raise ValueError("short smoke disk read")
        return data

    def write(self, offset: int, data: bytes) -> None:
        self.file.seek(offset)
        self.file.write(data)

    def cluster_offset(self, cluster: int) -> int:
        if cluster < 2:
            raise ValueError("invalid smoke fixture cluster")
        offset = (self.data_sector + (cluster - 2) * self.sectors_per_cluster) * SECTOR_SIZE
        if offset + self.cluster_bytes > self.disk_size:
            raise ValueError("smoke fixture cluster outside disk")
        return offset

    def fat_entry(self, cluster: int) -> int:
        offset = self.reserved * SECTOR_SIZE + cluster * 4
        return _u32(self.read(offset, 4), 0) & 0x0FFF_FFFF

    def set_fat_entry(self, cluster: int, value: int) -> None:
        for fat_index in range(self.fat_count):
            offset = (self.reserved + fat_index * self.fat_sectors) * SECTOR_SIZE + cluster * 4
            self.write(offset, struct.pack("<I", value))

    def root_entry(self, name83: bytes) -> tuple[int, int]:
        cluster = self.root_cluster
        seen: set[int] = set()
        for _ in range(64):
            if cluster in seen:
                break
            seen.add(cluster)
            contents = self.read(self.cluster_offset(cluster), self.cluster_bytes)
            for offset in range(0, len(contents), 32):
                entry = contents[offset : offset + 32]
                if entry[0] == 0:
                    break
                if entry[0] == 0xE5 or entry[11] == 0x0F or entry[11] & 0x08:
                    continue
                if entry[:11] == name83:
                    first = (_u16(entry, 20) << 16) | _u16(entry, 26)
                    return first, _u32(entry, 28)
            else:
                next_cluster = self.fat_entry(cluster)
                if 2 <= next_cluster < 0x0FFF_FFF8:
                    cluster = next_cluster
                    continue
            break
        raise ValueError(f"smoke fixture {name83!r} missing from root")


def prepare_fixtures(image: Path, hello_image: Path, scratch: Path) -> None:
    broken = scratch / "broken.txt"
    broken.write_bytes((b"Cantaya FAT chain fixture.\n" * 400)[:8192])
    for source, destination in (
        (broken, "BROKEN.TXT"),
        (hello_image, "BAD.ELF"),
        (hello_image, "BADWX.ELF"),
    ):
        subprocess.run(
            ["mcopy", "-i", str(image), str(source), f"::/{destination}"],
            check=True,
            capture_output=True,
        )

    volume = FatImage(image)
    try:
        broken_cluster, broken_size = volume.root_entry(b"BROKEN  TXT")
        if broken_size <= volume.cluster_bytes or volume.fat_entry(broken_cluster) < 2:
            raise ValueError("broken file did not receive a multi-cluster chain")
        volume.set_fat_entry(broken_cluster, broken_cluster)

        bad_cluster, bad_size = volume.root_entry(b"BAD     ELF")
        bad_offset = volume.cluster_offset(bad_cluster)
        if bad_size < 64 or volume.read(bad_offset, 4) != b"\x7fELF":
            raise ValueError("bad ELF fixture source is invalid")
        volume.write(bad_offset, b"\x00")

        wx_cluster, wx_size = volume.root_entry(b"BADWX   ELF")
        wx_offset = volume.cluster_offset(wx_cluster)
        header = volume.read(wx_offset, min(wx_size, volume.cluster_bytes))
        if header[:4] != b"\x7fELF":
            raise ValueError("W+X fixture source is invalid")
        program_offset = struct.unpack_from("<Q", header, 32)[0]
        header_size = _u16(header, 54)
        header_count = _u16(header, 56)
        if header_size != 56 or header_count == 0:
            raise ValueError("unexpected ELF program-header layout")
        for index in range(header_count):
            offset = program_offset + index * header_size
            if offset + header_size > len(header):
                raise ValueError("ELF program headers cross the first cluster")
            if _u32(header, offset) == 1:
                flags = _u32(header, offset + 4)
                volume.write(wx_offset + offset + 4, struct.pack("<I", flags | 0x3))
                break
        else:
            raise ValueError("ELF fixture has no PT_LOAD segment")
    finally:
        volume.close()
