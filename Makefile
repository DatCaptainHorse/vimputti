CC = gcc
CFLAGS = -Wall -O2

test_shim: examples/test_shim.c
	$(CC) $(CFLAGS) -o test_shim examples/test_shim.c

clean:
	rm -f test_shim

.PHONY: clean
