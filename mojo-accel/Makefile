# mojo-accel/Makefile
# WSL 経由で Mojo をビルド
#
# 使用方法:
#   make all          — 全ターゲットをビルド
#   make clean        — ビルド成果物を削除
#   make <target>     — 個別ターゲットをビルド

MOJO_ENV = source ~/mojo-env/bin/activate
WSL = wsl -e bash -c
MOJO_DIR = /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel

# ビルドターゲット一覧
TARGETS = flash_attention kv_quantizer vt_parser paged_attention async_io

.PHONY: all clean $(TARGETS)

all: $(TARGETS)
	@echo "✓ 全 Mojo モジュールのビルド完了"

flash_attention:
	$(WSL) "$(MOJO_ENV) && cd $(MOJO_DIR) && mojo build flash_attention_simd.mojo -o flash_attention_simd"

kv_quantizer:
	$(WSL) "$(MOJO_ENV) && cd $(MOJO_DIR) && mojo build kv_quantizer_simd.mojo -o kv_quantizer_simd"

vt_parser:
	$(WSL) "$(MOJO_ENV) && cd $(MOJO_DIR) && mojo build vt_parser_simd.mojo -o vt_parser_simd"

paged_attention:
	$(WSL) "$(MOJO_ENV) && cd $(MOJO_DIR) && mojo build paged_attention_simd.mojo -o paged_attention_simd"

async_io:
	$(WSL) "$(MOJO_ENV) && cd $(MOJO_DIR) && mojo build async_io_simd.mojo -o async_io_simd"

clean:
	$(WSL) "cd $(MOJO_DIR) && rm -f flash_attention_simd kv_quantizer_simd vt_parser_simd paged_attention_simd async_io_simd"
	@echo "✓ ビルド成果物を削除"
