#!/usr/bin/env python3
"""Write Lucidos's Finder layout using a buddy allocator and one B-tree leaf."""

import argparse
import plistlib
import struct
from pathlib import Path

WINDOW = {
    'ShowStatusBar': False, 'ShowToolbar': False, 'ShowTabView': False,
    'ContainerShowSidebar': False, 'ShowSidebar': False,
    'WindowBounds': '{{10, 980}, {660, 400}}',
}
ICON_VIEW = {
    'backgroundColorRed': 1.0, 'backgroundColorGreen': 1.0,
    'backgroundColorBlue': 1.0, 'showIconPreview': True, 'textSize': 16.0,
    'backgroundType': 0, 'gridOffsetX': 0.0, 'gridOffsetY': 0.0,
    'showItemInfo': False, 'viewOptionsVersion': 1, 'arrangeBy': 'none',
    'labelOnBottom': True, 'iconSize': 128.0, 'gridSpacing': 100.0,
}
POSITIONS = {'Applications': (480, 170), 'Lucidos.app': (180, 170)}


def pack(*values):
    return struct.pack('>' + 'I' * len(values), *values)


def layout_bytes():
    """Emit deterministic records matching the shipped Finder window."""
    records = []

    def record(name, code, kind, value):
        encoded = name.encode('utf-16-be')
        payload = pack(len(value)) + value if kind == b'blob' else pack(value)
        records.append(((name.lower(), code), pack(len(encoded) // 2) + encoded
                        + code + kind + payload))

    for code, value in ((b'bwsp', WINDOW), (b'icvp', ICON_VIEW)):
        record('.', code, b'blob', plistlib.dumps(
            value, fmt=plistlib.FMT_BINARY, sort_keys=True))
    record('.', b'vSrn', b'long', 1)
    for name, position in POSITIONS.items():
        record(name, b'Iloc', b'blob', pack(*position) + b'\xff' * 6 + b'\0\0')
    records.sort()
    leaf = pack(0, len(records)) + b''.join(value for _, value in records)

    free = [[] for _ in range(32)]
    free[31] = [0]

    def allocate(size):
        width = max(5, (size - 1).bit_length())
        level = width
        while not free[level]:
            level += 1
        offset = free[level].pop(0)
        while level > width:
            level -= 1
            free[level].append(offset + (1 << level))
            free[level].sort()
        return offset | width, offset

    allocate(32)  # Reserve the allocator header.
    database_address, database_offset = allocate(20)
    leaf_address, leaf_offset = allocate(len(leaf))
    addresses = [0, database_address, leaf_address]

    def allocator_root():
        # Block addresses are padded to a multiple of 256 entries.
        result = pack(len(addresses), 0, *addresses) + b'\0' * (4 * (256 - len(addresses)))
        result += pack(1) + b'\x04DSDB' + pack(1)
        for offsets in free:
            result += pack(len(offsets), *offsets)
        return result

    # Splitting a buddy adds at most one entry per level to the free lists.
    root_address, root_offset = allocate(len(allocator_root()) + 32 * 4)
    addresses[0] = root_address
    root = allocator_root()
    root_size = 1 << (root_address & 31)
    assert len(root) <= root_size
    result = bytearray(root_offset + root_size)
    result[:16] = b'Bud1' + pack(root_offset, root_size, root_offset)
    result[database_offset:database_offset + 20] = pack(2, 0, len(records), 1, 4096)
    result[leaf_offset:leaf_offset + len(leaf)] = leaf
    result[root_offset:root_offset + len(root)] = root
    return pack(1) + result


def read_layout(path):
    """Read the single-leaf layout, including Finder-written reference files."""
    data = Path(path).read_bytes()
    if data[:8] != pack(1) + b'Bud1':
        raise ValueError('Invalid DS_Store header')
    data = data[4:]
    root_offset, root_size, repeated = struct.unpack_from('>III', data, 4)
    if root_offset != repeated:
        raise ValueError('Inconsistent allocator root')
    root = data[root_offset:root_offset + root_size]
    count = struct.unpack_from('>I', root)[0]
    addresses = struct.unpack_from('>' + 'I' * count, root, 8)
    cursor = 8 + 4 * ((count + 255) // 256 * 256)
    entries = struct.unpack_from('>I', root, cursor)[0]
    cursor += 4
    toc = {}
    for _ in range(entries):
        length = root[cursor]
        name = root[cursor + 1:cursor + 1 + length]
        cursor += 1 + length
        toc[name] = struct.unpack_from('>I', root, cursor)[0]
        cursor += 4
    database = addresses[toc[b'DSDB']] & ~31
    leaf_index, levels, total, nodes, _ = struct.unpack_from('>IIIII', data, database)
    if levels != 0 or nodes != 1:
        raise ValueError('Expected a single-leaf layout')
    cursor = addresses[leaf_index] & ~31
    child, count = struct.unpack_from('>II', data, cursor)
    if child != 0 or count != total:
        raise ValueError('Invalid leaf')
    cursor += 8
    records = {}
    for _ in range(count):
        length = struct.unpack_from('>I', data, cursor)[0]
        cursor += 4
        name = data[cursor:cursor + length * 2].decode('utf-16-be')
        cursor += length * 2
        code, kind = data[cursor:cursor + 4], data[cursor + 4:cursor + 8]
        cursor += 8
        value = struct.unpack_from('>I', data, cursor)[0]
        cursor += 4
        if kind == b'blob':
            length = value
            value = data[cursor:cursor + length]
            cursor += length
            if value.startswith(b'bplist'):
                value = plistlib.loads(value)
            elif code == b'Iloc':
                value = struct.unpack('>II8s', value)
        elif kind != b'long':
            raise ValueError(f'Unsupported record type: {kind!r}')
        records[(name, code.decode('ascii'))] = value
    return records


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('path', type=Path)
    parser.add_argument('--read', action='store_true')
    args = parser.parse_args()
    if args.read:
        for key, value in read_layout(args.path).items():
            print(key, value)
    else:
        args.path.write_bytes(layout_bytes())
