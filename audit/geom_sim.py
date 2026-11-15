"""Mirror of aimg src/ext4/write.rs geometry/layout math, to probe invariants.

Invariants checked per (block_size, image_size):
  G1 gdt_blocks must cover groups*desc_size bytes (div_ceil)
  G2 every group's [bb, ib, itable] must be inside the image and must not
     overlap the group's backup SB/GDT
  G3 group 0 block 0 must be marked used when first_data_block == 1
  G4 inode table blocks must be div_ceil(ipg*inode_size, bs) exactly covering ipg inodes
  G5 total inodes = ipg*groups, ipg >= 16
"""


def align_up(v, a):
    return (v + a - 1) // a * a


def sparse_super_group(g):
    def is_pow(n, base):
        if n < base:
            return False
        while n % base == 0:
            n //= base
        return n == 1
    if g in (0, 1):
        return True
    return is_pow(g, 3) or is_pow(g, 5) or is_pow(g, 7)


def analyze(bs, size, inode_size=256):
    problems = []
    size = align_up(size, bs)
    blocks = size // bs
    fdb = 1 if bs == 1024 else 0
    bpg = 8 * bs
    groups = max(1, -(-(blocks - fdb) // bpg))
    ipg = 8192 if bs >= 4096 else 2048
    itable_blocks = max(1, (ipg * inode_size) // bs)
    desc_size = 32
    # ---- as written in src (integer division!) ----
    gdt_blocks = max(1, (groups * desc_size) // bs)
    gdt_need = max(1, -(-(groups * desc_size) // bs))
    if gdt_blocks != gdt_need:
        problems.append(
            f"G1 gdt_blocks={gdt_blocks} but descriptor table needs {gdt_need} blocks "
            f"(groups={groups}, desc={desc_size}B) -> primary/backup GDT overruns the "
            f"block bitmap of group 0 / backup groups"
        )
    last_g = groups - 1
    last_start = fdb + last_g * bpg
    last_hdr = (1 + gdt_need) if sparse_super_group(last_g) else 0
    last_avail = (blocks - last_start) - (last_hdr + 2)
    max_ipg = (last_avail * bs) // inode_size
    if max_ipg < 16:
        return problems + [f"G5 image too small for a final group ({last_avail} blocks)"]
    if max_ipg < ipg:
        ipg = max_ipg
        itable_blocks = max(1, (ipg * inode_size) // bs)
        if ipg * inode_size > itable_blocks * bs:
            problems.append(
                f"G4 itable_blocks={itable_blocks} < ceil({ipg}*{inode_size}/{bs})"
            )
    total_inodes = ipg * groups
    super_blocks = 1 + gdt_need

    used = set()
    if fdb == 1 and 0 not in used:
        problems.append("G3 1 KiB blocks: block 0 (boot block) is never marked used")

    def mark(b):
        used.add(b)

    g0_start = fdb
    for i in range(super_blocks):
        mark(g0_start + i)
    g0_bb = g0_start + super_blocks
    g0_ib = g0_bb + 1
    g0_it = g0_ib + 1
    mark(g0_bb); mark(g0_ib)
    for i in range(itable_blocks):
        mark(g0_it + i)
    if g0_bb < g0_start + super_blocks:
        problems.append("G2 group0 bitmap overlaps GDT")

    layout = [(0, g0_bb, g0_ib, g0_it)]
    for g in range(1, groups):
        start = fdb + g * bpg
        hdr = (1 + gdt_need) if sparse_super_group(g) else 0
        bb = start + hdr
        ib = bb + 1
        it = ib + 1
        # what the code actually does (uses its own gdt_blocks)
        bb_code = start + (1 + gdt_blocks if sparse_super_group(g) else 0)
        if bb_code != bb:
            problems.append(
                f"G2 group {g}: code places block bitmap at {bb_code}, "
                f"must be {bb} (backup SB/GDT overlap)"
            )
        for b in (bb, ib):
            if b >= blocks:
                problems.append(f"G2 group {g}: bitmap block {b} past image end {blocks}")
            mark(b)
        for i in range(itable_blocks):
            if it + i >= blocks:
                problems.append(f"G2 group {g}: inode table block {it+i} past image end")
            mark(it + i)
        layout.append((g, bb, ib, it))
        if sparse_super_group(g) and bb < start + super_blocks:
            problems.append(f"G2 group {g}: bitmap overlaps backup SB/GDT")

    free = blocks - len(used)
    return problems, dict(
        blocks=blocks,
        groups=groups,
        ipg=ipg,
        itable_blocks=itable_blocks,
        total_inodes=total_inodes,
        gdt_blocks=gdt_blocks,
        gdt_need=gdt_need,
        used=len(used),
        free=free,
        last_avail=last_avail,
    )


def main():
    for bs in (1024, 2048, 4096):
        print(f"########## block_size={bs} ##########")
        prev = None
        for mib in [4, 8, 16, 32, 64, 128, 130, 132, 256, 264, 265, 512, 1024, 2048,
                    4096, 8192, 16384, 17000, 20000, 33000, 66000]:
            size = mib * 1024 * 1024
            r = analyze(bs, size)
            if isinstance(r, list):
                print(f"{mib:>6} MiB: BAIL {r}")
                continue
            probs, info = r
            key = (tuple(sorted(set(p.split(':')[0].split()[0] for p in probs))),)
            if probs:
                print(f"{mib:>6} MiB: groups={info['groups']:>3} ipg={info['ipg']:>5} "
                      f"gdt={info['gdt_blocks']}/{info['gdt_need']} PROBLEMS:")
                for p in probs:
                    print(f"          - {p}")
            else:
                print(f"{mib:>6} MiB: groups={info['groups']:>3} ipg={info['ipg']:>5} "
                      f"gdt={info['gdt_blocks']} ok")
            prev = info


if __name__ == "__main__":
    main()
