/*
 * Oak Video Editor - Non-Linear Video Editor
 * Copyright (C) 2026 Oak Team
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */

/*
 * OFX Message suite 的 C 入口。stable Rust 无法定义 C-variadic 函数
 * （c_variadic 仍不稳定）：v1 的 '...' 与 v2 的 va_list 都在这里
 * vsnprintf 成定长缓冲，再转发给 Rust 实现 oak_ofx_message_impl。
 *
 * 编译：build.rs（cc crate）；符号随 staticlib 进入 liboakplugin
 * （corrosion 链接期无需额外接线）。
 */

#include <stdarg.h>
#include <stdio.h>

extern int oak_ofx_message_impl(void *handle, const char *type,
                                const char *id, const char *message);

/* 消息缓冲上限（镜像 C++ 侧 format_message 的 1024 惯例，
 * olivehost.cpp:99；超长截断不报错）。 */
#define OAK_MSG_BUF_SIZE 1024

static int forward(void *handle, const char *type, const char *id,
                   const char *format, va_list args)
{
    if (!format) {
        /* C++ 侧 !format → kOfxStatFailed（olivehost.cpp:267）；
         * 以 NULL message 通知 Rust 侧。 */
        return oak_ofx_message_impl(handle, type, id, NULL);
    }

    char buf[OAK_MSG_BUF_SIZE];
    int n = vsnprintf(buf, sizeof(buf), format, args);
    if (n < 0) {
        return oak_ofx_message_impl(handle, type, id, NULL);
    }
    return oak_ofx_message_impl(handle, type, id, buf);
}

int ofx_message_shim_v1(void *handle, const char *type, const char *id,
                        const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int r = forward(handle, type, id, format, args);
    va_end(args);
    return r;
}

int ofx_message_shim_v2(void *handle, const char *type, const char *id,
                        const char *format, va_list args)
{
    return forward(handle, type, id, format, args);
}
