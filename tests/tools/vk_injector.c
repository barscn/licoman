/*
 * vk_injector — minimal Wayland virtual-keyboard key injector.
 *
 * **STATUS: WIP — NOT YET FUNCTIONAL (2026-10-30).** It connects, enumerates
 * globals, binds wl_seat + zwp_virtual_keyboard_manager_v1 and uploads its
 * keymap, but this Hyprland build (0.56.2) drops the connection right after
 * the registry binds, before any key is delivered. It also rejects
 * `wl_display.sync` from hand-rolled clients, so the handshake reads globals
 * with a timeout instead of a roundtrip. Kept as the starting point for
 * Phase 02+ keyboard verification; see docs/design/decisions.md D12. The
 * Hyprland `sendshortcut` dispatcher was also tried first: it reports ok but
 * synthesized keys never reach the gpui client.
 *
 * Test harness for Licoman's keyboard-verification gates (Phase 01):
 * no keyboard-injection tool (wtype/ydotool/wlrctl) was available on the
 * development machine, so this client creates a zwp_virtual_keyboard_v1
 * (the same mechanism wtype uses), uploads a US layout keymap and injects
 * the key sequence given on the command line into the currently focused
 * Wayland surface.
 *
 * Build:  cc tests/tools/vk_injector.c -o /tmp/vk -lxkbcommon
 * Usage:  /tmp/vk combo:CTRL+k type:power key:Return key:Escape key:Down
 *         /tmp/vk combo:CTRL+comma key:Tab key:Space
 *
 * Key names: letters a-z, digits, comma, period, space, Return, Escape,
 * Tab, Backspace, Up, Down, Left, Right. Modifiers: CTRL, SHIFT, ALT, SUPER.
 *
 * Wayland wire notes: messages are [object u32][(size<<16)|op u32][args];
 * strings are length-prefixed (including NUL) and padded to 4 bytes; the
 * keymap request carries its fd out-of-band via SCM_RIGHTS.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <xkbcommon/xkbcommon.h>

#define ID_DISPLAY 1
#define ID_REGISTRY 2

static int sock = -1;
static uint32_t next_id = 10;

/* ---- wire helpers ------------------------------------------------------ */

static void put_u32(uint8_t **p, uint32_t v) {
    memcpy(*p, &v, 4);
    *p += 4;
}

static void put_string(uint8_t **p, const char *s) {
    uint32_t len = (uint32_t)strlen(s) + 1;
    put_u32(p, len);
    memcpy(*p, s, len);
    *p += len;
    while ((size_t)(*p) % 4)
        (*p)++;
}

static size_t string_wire_len(const char *s) {
    uint32_t len = (uint32_t)strlen(s) + 1;
    return 4 + len + ((4 - (len % 4)) % 4);
}

static void send_msg(uint8_t *buf, size_t len, int fd_to_pass) {
    struct iovec iov = {.iov_base = buf, .iov_len = len};
    struct msghdr mh = {.msg_iov = &iov, .msg_iovlen = 1};
    char cmsg[CMSG_SPACE(sizeof(int))];
    if (fd_to_pass >= 0) {
        memset(cmsg, 0, sizeof(cmsg));
        mh.msg_control = cmsg;
        mh.msg_controllen = sizeof(cmsg);
        struct cmsghdr *cm = CMSG_FIRSTHDR(&mh);
        cm->cmsg_level = SOL_SOCKET;
        cm->cmsg_type = SCM_RIGHTS;
        cm->cmsg_len = CMSG_LEN(sizeof(int));
        memcpy(CMSG_DATA(cm), &fd_to_pass, sizeof(int));
    }
    if (sendmsg(sock, &mh, MSG_NOSIGNAL) < 0) {
        perror("sendmsg");
        exit(1);
    }
    if (getenv("VK_DEBUG")) {
        fprintf(stderr, "send %zu bytes:", len);
        for (size_t i = 0; i < len; i++)
            fprintf(stderr, " %02x", buf[i]);
        fprintf(stderr, "\n");
    }
}

/* ---- event reading (only what the handshake needs) --------------------- */

struct global {
    char interface[64];
    uint32_t name;
    uint32_t version;
};

static struct global globals[256];
static int n_globals = 0;

static void parse_string(uint8_t **p, char *out, size_t outlen) {
    uint32_t len;
    memcpy(&len, *p, 4);
    *p += 4;
    size_t copy = len < outlen ? len - 1 : outlen - 1;
    memcpy(out, *p, copy);
    out[copy] = 0;
    *p += len;
    while ((uintptr_t)(*p) % 4)
        (*p)++;
}

static void roundtrip_variant(int mode) {
    /* mode 0: sync, size<<16|op, new_id 10
     * mode 1: no sync — read globals until a short timeout
     * mode 2: sync, opcode<<16|size encoding
     * mode 3: sync as mode 0 but callback id 3 */
    uint32_t cb = mode == 3 ? 3 : next_id++;
    if (mode != 1) {
        uint8_t buf[16];
        uint8_t *w = buf;
        put_u32(&w, ID_DISPLAY);
        put_u32(&w, mode == 2 ? (0u << 16) | 12u : (12u << 16) | 0u);
        put_u32(&w, cb);
        send_msg(buf, 12, -1);
    } else {
        struct timeval tv = {.tv_sec = 0, .tv_usec = 400000};
        setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    }

    uint8_t rx[8192];
    for (;;) {
        ssize_t n = recv(sock, rx, sizeof(rx), 0);
        if (n <= 0) {
            if (mode == 1 && (n < 0 && errno == EAGAIN)) {
                fprintf(stderr, "globals-only: collected %d globals\n", n_globals);
                for (int i = 0; i < n_globals; i++)
                    fprintf(stderr, "  %u %s v%u\n", globals[i].name, globals[i].interface,
                            globals[i].version);
                return;
            }
            fprintf(stderr, "wayland read failed: n=%zd errno=%d (%s)\n", n, errno,
                    strerror(errno));
            exit(1);
        }
        uint8_t *p = rx;
        int done = 0;
        while ((size_t)(p - rx) + 8 <= (size_t)n) {
            uint32_t obj, size_op;
            memcpy(&obj, p, 4);
            memcpy(&size_op, p + 4, 4);
            uint32_t size = size_op >> 16, op = size_op & 0xffff;
            uint8_t *body = p + 8;
            if (obj == ID_DISPLAY && op == 0) {
                /* wl_display.error: object u32, code u32, message string */
                uint32_t eobj, ecode;
                char msg[256] = {0};
                uint8_t *q = body;
                memcpy(&eobj, q, 4);
                q += 4;
                memcpy(&ecode, q, 4);
                q += 4;
                parse_string(&q, msg, sizeof(msg));
                fprintf(stderr, "wayland protocol error on object %u code %u: %s\n", eobj,
                        ecode, msg);
                exit(5);
            } else if (obj == ID_REGISTRY && op == 0 && n_globals < 256) {
                /* global: name u32, interface string, version u32 */
                struct global *g = &globals[n_globals];
                memcpy(&g->name, body, 4);
                uint8_t *q = body + 4;
                parse_string(&q, g->interface, sizeof(g->interface));
                memcpy(&g->version, q, 4);
                n_globals++;
            } else if (obj == cb && op == 0) {
                done = 1;
            }
            p += size;
        }
        if (done)
            return;
    }
}

/* ---- keyboard state ---------------------------------------------------- */

static uint32_t mod_depressed = 0;
#define MOD_SHIFT 1u
#define MOD_CTRL 4u
#define MOD_ALT 8u
#define MOD_SUPER 64u

struct keydef {
    const char *name;
    uint32_t code; /* Linux evdev code; Wayland uses evdev + 8 */
};

static const struct keydef keys[] = {
    {"a", 30},  {"b", 48},  {"c", 46}, {"d", 32},  {"e", 18}, {"f", 33},
    {"g", 34},  {"h", 35},  {"i", 23}, {"j", 36},  {"k", 37}, {"l", 38},
    {"m", 39},  {"n", 49},  {"o", 24}, {"p", 25},  {"q", 16}, {"r", 19},
    {"s", 31},  {"t", 20},  {"u", 22}, {"v", 47},  {"w", 17}, {"x", 45},
    {"y", 21},  {"z", 44},  {"1", 2},  {"2", 3},   {"3", 4},  {"4", 5},
    {"5", 6},   {"6", 7},   {"7", 8},  {"8", 9},   {"9", 10}, {"0", 11},
    {"comma", 51},   {"period", 52}, {"space", 57}, {"Return", 28},
    {"Escape", 1},   {"Tab", 15},    {"Backspace", 14}, {"Up", 103},
    {"Down", 108},   {"Left", 105},  {"Right", 106},    {"minus", 12},
    {"equal", 13},   {"slash", 53},
};

static struct {
    const char *name;
    uint32_t code, mask;
} mods[] = {
    {"CTRL", 29, MOD_CTRL},
    {"SHIFT", 42, MOD_SHIFT},
    {"ALT", 56, MOD_ALT},
    {"SUPER", 125, MOD_SUPER},
};

static uint32_t kb_id;

static void send_key(uint32_t evdev, int press) {
    uint8_t buf[20];
    uint8_t *w = buf;
    static uint32_t t = 100;
    t += 20;
    put_u32(&w, kb_id);
    put_u32(&w, (20u << 16) | 1u); /* key */
    put_u32(&w, t);
    put_u32(&w, evdev + 8);
    put_u32(&w, press ? 1u : 0u);
    send_msg(buf, 20, -1);
}

static void send_modifiers(void) {
    uint8_t buf[28];
    uint8_t *w = buf;
    static uint32_t serial = 1;
    put_u32(&w, kb_id);
    put_u32(&w, (28u << 16) | 2u); /* modifiers */
    put_u32(&w, serial++);
    put_u32(&w, mod_depressed);
    put_u32(&w, 0);
    put_u32(&w, 0);
    put_u32(&w, 0);
    send_msg(buf, 28, -1);
}

static uint32_t lookup_key(const char *name) {
    for (size_t i = 0; i < sizeof(keys) / sizeof(keys[0]); i++)
        if (strcmp(keys[i].name, name) == 0)
            return keys[i].code;
    for (size_t i = 0; i < sizeof(mods) / sizeof(mods[0]); i++)
        if (strcmp(mods[i].name, name) == 0)
            return mods[i].code;
    fprintf(stderr, "vk_injector: unknown key '%s'\n", name);
    exit(2);
}

static int lookup_mod(const char *name) {
    for (size_t i = 0; i < sizeof(mods) / sizeof(mods[0]); i++)
        if (strcmp(mods[i].name, name) == 0)
            return (int)i;
    return -1;
}

static void tap(uint32_t code) {
    send_key(code, 1);
    send_modifiers();
    usleep(25000);
    send_key(code, 0);
    send_modifiers();
    usleep(25000);
}

static void press_mod(int mi) {
    send_key(mods[mi].code, 1);
    mod_depressed |= mods[mi].mask;
    send_modifiers();
    usleep(25000);
}

static void release_mod(int mi) {
    send_key(mods[mi].code, 0);
    mod_depressed &= ~mods[mi].mask;
    send_modifiers();
    usleep(25000);
}

static void run_token(char *tok) {
    if (strncmp(tok, "type:", 5) == 0) {
        for (char *c = tok + 5; *c; c++) {
            char name[2] = {*c, 0};
            if (*c == ' ') {
                tap(57);
                continue;
            }
            if (*c >= 'A' && *c <= 'Z') {
                name[0] = (char)(*c + 32);
                press_mod(lookup_mod("SHIFT"));
                tap(lookup_key(name));
                release_mod(lookup_mod("SHIFT"));
                continue;
            }
            tap(lookup_key(name));
        }
        return;
    }
    if (strncmp(tok, "combo:", 6) == 0) {
        int mi[4], nmi = 0;
        char *keyname = tok + 6;
        char *plus;
        while ((plus = strchr(keyname, '+'))) {
            *plus = 0;
            int m = lookup_mod(keyname);
            if (m < 0) {
                fprintf(stderr, "vk_injector: unknown modifier '%s'\n", keyname);
                exit(2);
            }
            mi[nmi++] = m;
            keyname = plus + 1;
        }
        for (int i = 0; i < nmi; i++)
            press_mod(mi[i]);
        tap(lookup_key(keyname));
        for (int i = nmi - 1; i >= 0; i--)
            release_mod(mi[i]);
        return;
    }
    if (strncmp(tok, "key:", 4) == 0) {
        tap(lookup_key(tok + 4));
        return;
    }
    fprintf(stderr, "vk_injector: bad token '%s' (want key:|combo:|type:)\n", tok);
    exit(2);
}

/* ---- connection -------------------------------------------------------- */

int main(int argc, char **argv) {
    const char *runtime = getenv("XDG_RUNTIME_DIR");
    const char *display = getenv("WAYLAND_DISPLAY");
    if (!runtime || !display) {
        fprintf(stderr, "vk_injector: need XDG_RUNTIME_DIR and WAYLAND_DISPLAY\n");
        return 2;
    }
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    snprintf(addr.sun_path, sizeof(addr.sun_path), "%s/%s", runtime, display);
    sock = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (connect(sock, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("connect");
        return 1;
    }

    /* registry + initial roundtrip */
    uint8_t buf[32];
    uint8_t *w = buf;
    put_u32(&w, ID_DISPLAY);
    put_u32(&w, (12u << 16) | 1u); /* get_registry */
    put_u32(&w, ID_REGISTRY);
    send_msg(buf, 12, -1);
    /* This Hyprland build rejects wl_display.sync, so the default handshake
     * reads globals until a short timeout instead of doing a roundtrip. */
    roundtrip_variant(getenv("VK_MODE") ? atoi(getenv("VK_MODE")) : 1);

    uint32_t seat_name = 0, seat_version = 0, manager_name = 0;
    for (int i = 0; i < n_globals; i++) {
        if (strcmp(globals[i].interface, "wl_seat") == 0 && !seat_name) {
            seat_name = globals[i].name;
            seat_version = globals[i].version;
        }
        if (strcmp(globals[i].interface, "zwp_virtual_keyboard_manager_v1") == 0)
            manager_name = globals[i].name;
    }
    if (!seat_name || !manager_name) {
        fprintf(stderr, "vk_injector: compositor lacks wl_seat (%u) or "
                        "zwp_virtual_keyboard_manager_v1 (%u)\n",
                seat_name, manager_name);
        return 3;
    }

    uint32_t seat_id = next_id++;
    uint32_t manager_id = next_id++;
    uint32_t bind_ver = seat_version < 5 ? seat_version : 5;
    uint8_t b[128];
    w = b;
    put_u32(&w, ID_REGISTRY);
    put_u32(&w, (uint32_t)(((8 + 4 + string_wire_len("wl_seat") + 4 + 4)) << 16) | 0u);
    put_u32(&w, seat_name);
    put_string(&w, "wl_seat");
    put_u32(&w, bind_ver);
    put_u32(&w, seat_id);
    send_msg(b, (size_t)(w - b), -1);

    w = b;
    put_u32(&w, ID_REGISTRY);
    put_u32(&w, (uint32_t)((8 + 4 + string_wire_len("zwp_virtual_keyboard_manager_v1") + 4 + 4) << 16) | 0u);
    put_u32(&w, manager_name);
    put_string(&w, "zwp_virtual_keyboard_manager_v1");
    put_u32(&w, 1);
    put_u32(&w, manager_id);
    send_msg(b, (size_t)(w - b), -1);

    kb_id = next_id++;
    w = b;
    put_u32(&w, manager_id);
    put_u32(&w, (12u << 16) | 0u); /* create_virtual_keyboard */
    put_u32(&w, seat_id);
    put_u32(&w, kb_id);
    send_msg(b, 16, -1);

    /* compile and upload a US keymap so keysyms resolve predictably */
    struct xkb_context *xctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    struct xkb_rule_names names = {.rules = "evdev", .model = "pc105", .layout = "us"};
    struct xkb_keymap *xmap = xkb_keymap_new_from_names(xctx, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
    if (!xmap) {
        fprintf(stderr, "vk_injector: failed to compile keymap\n");
        return 4;
    }
    const char *km = xkb_keymap_get_as_string(xmap, XKB_KEYMAP_FORMAT_TEXT_V1);
    size_t km_len = strlen(km);
    int km_fd = memfd_create("licoman-vk-keymap", 0);
    if (ftruncate(km_fd, (off_t)km_len) < 0 || write(km_fd, km, km_len) != (ssize_t)km_len) {
        perror("keymap fd");
        return 4;
    }
    w = b;
    put_u32(&w, kb_id);
    put_u32(&w, (20u << 16) | 0u); /* keymap(format, fd, size) */
    put_u32(&w, 1);                /* XKB v1 */
    put_u32(&w, km_len);
    send_msg(b, 20, km_fd);
    close(km_fd);

    for (int i = 1; i < argc; i++)
        run_token(argv[i]);
    usleep(50000);
    close(sock);
    xkb_keymap_unref(xmap);
    xkb_context_unref(xctx);
    return 0;
}
