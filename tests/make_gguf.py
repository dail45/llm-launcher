import struct, sys
def s(b, x): x = x.encode(); b += struct.pack('<Q', len(x)) + x
def kv_str(b, k, v): s(b, k); b += struct.pack('<I', 8); s(b, v)
def kv_u32(b, k, v): s(b, k); b += struct.pack('<I', 4) + struct.pack('<I', v)
def make(path, layers=4, embd=256, vocab=1000, name="tiny"):
    b = bytearray(b'GGUF') + struct.pack('<IQQ', 3, 2 + layers, 9)
    kv_str(b, "general.architecture", "llama"); kv_str(b, "general.name", name)
    kv_u32(b, "general.file_type", 15); kv_u32(b, "llama.block_count", layers)
    kv_u32(b, "llama.embedding_length", embd); kv_u32(b, "llama.attention.head_count", 8)
    kv_u32(b, "llama.attention.head_count_kv", 2); kv_u32(b, "llama.context_length", 8192)
    kv_u32(b, "llama.vocab_size", vocab)
    tensors = [("token_embd.weight", [embd, vocab], 1), ("output.weight", [embd, vocab], 1)] + [(f"blk.{i}.attn_q.weight", [embd, embd], 1) for i in range(layers)]
    off = 0; sizes = []
    for n, d, t in tensors:
        s(b, n); b += struct.pack('<I', len(d)); [b.__iadd__(struct.pack('<Q', x)) for x in d]
        b += struct.pack('<IQ', t, off); sz = d[0] * d[1] * 2; sizes.append(sz); off += (sz + 31) // 32 * 32
    while len(b) % 32: b.append(0)
    b += bytes(off)
    open(path, 'wb').write(b)
make(sys.argv[1])
