# Vendored embedding model — all-MiniLM-L6-v2 (int8)

Used by `src/embed.rs` for local RAG retrieval in the `:chat` feature. Embedded
into the binary via `include_bytes!`, so the embedder is fully offline. Stored
in Git LFS (see `.gitattributes`).

## Files

| File | Source | Notes |
|------|--------|-------|
| `model_int8.onnx` | [Xenova/all-MiniLM-L6-v2](https://huggingface.co/Xenova/all-MiniLM-L6-v2) → `onnx/model_quantized.onnx` | int8 dynamic-quantized ONNX export (~22 MB; fp32 would be ~90 MB). 384-dim sentence embeddings. |
| `tokenizer.json` | same repo → `tokenizer.json` | BERT WordPiece tokenizer. |

## License

Apache-2.0 (the `all-MiniLM-L6-v2` model and the Xenova ONNX export are both
Apache-2.0), which permits redistribution. The original model is from
sentence-transformers (`sentence-transformers/all-MiniLM-L6-v2`).

## Checksums (SHA-256)

```
afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1  model_int8.onnx
da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0  tokenizer.json
```

Verify with `sha256sum -c` after pulling LFS content.
