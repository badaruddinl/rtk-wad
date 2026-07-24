import sys

import tiktoken


encoding = tiktoken.get_encoding("o200k_base")
text = sys.stdin.buffer.read().decode("utf-8", errors="replace")
print(len(encoding.encode(text)))
