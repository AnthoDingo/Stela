// SPDX-License-Identifier: GPL-2.0-only
/* What Linux's text buffers need beyond fs/seq_output.c, following Linux
 * cc9de5d4b5ab6459f5774946aa3195ad51236fa8 lib/seq_buf.c and fs/seq_file.c. */
#include <linux/seq_buf.h>
#include <linux/seq_file.h>
#include <linux/sprintf.h>
#include <linux/string.h>

int seq_buf_printf(struct seq_buf *s, const char *fmt, ...)
{
	va_list arguments;
	int written = -1;

	va_start(arguments, fmt);
	if (s->len < s->size) {
		int length = vsnprintf(s->buffer + s->len, s->size - s->len, fmt, arguments);

		if (s->len + (size_t)length < s->size) {
			s->len += (size_t)length;
			written = 0;
		}
	}
	va_end(arguments);
	if (written)
		seq_buf_set_overflow(s);
	return written;
}

/* The string with its terminator, which the length does not count. */
int seq_buf_puts(struct seq_buf *s, const char *str)
{
	size_t length = strlen(str) + 1;

	WARN_ON(s->size == 0);
	if (s->len + length <= s->size) {
		memcpy(s->buffer + s->len, str, length);
		s->len += length - 1;
		return 0;
	}
	seq_buf_set_overflow(s);
	return -1;
}

void seq_putc(struct seq_file *m, char c)
{
	if (m->count < m->size)
		m->buf[m->count++] = c;
}

#ifdef STELA_LINUX_SEQ_TEST
#include <stela/test.h>

int main(void)
{
	char text[8];
	struct seq_buf buffer;

	seq_buf_init(&buffer, text, sizeof(text));
	assert(!seq_buf_puts(&buffer, "[C]") && seq_buf_used(&buffer) == 3);
	assert(!seq_buf_printf(&buffer, "[%d]", 1) && !strcmp(text, "[C][1]"));
	/* Seven characters and the terminator fit; one more does not. */
	assert(!seq_buf_puts(&buffer, "x") && !strcmp(text, "[C][1]x"));
	assert(seq_buf_puts(&buffer, "y") == -1 && seq_buf_has_overflowed(&buffer));
	printf("Linux's text buffers passed\n");
	return 0;
}
#endif
