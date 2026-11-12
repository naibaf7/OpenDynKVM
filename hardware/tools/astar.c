/* A* search kernel for tools/route.py (NetRouter.search), loaded with ctypes.
 *
 * Grid states are (layer L, row j, column i, direction d of the last step; 8 = none yet), ordered like the
 * Python tuples (f, g, L, j, i, d) so ties break the same way and both implementations return the same path.
 * ctypes releases the GIL around the call, so several Python threads can search at once.
 *
 * Build: gcc -O2 -shared -fPIC -o astar.so astar.c   (route.py does this on demand)
 */
#include <stdint.h>
#include <stdlib.h>

static const int DI[8] = {1, 1, 0, -1, -1, -1, 0, 1};
static const int DJ[8] = {0, 1, 1, 1, 0, -1, -1, -1};
static const double SQ2 = 1.4142135623730951;

typedef struct { double f, g; int64_t s; } node;
typedef struct { node *a; int64_t n, cap; } heap;

static int less(const node *x, const node *y) {
    if (x->f != y->f) return x->f < y->f;
    if (x->g != y->g) return x->g < y->g;
    return x->s < y->s;
}

static int push(heap *h, double f, double g, int64_t s) {
    if (h->n == h->cap) {
        int64_t cap = h->cap ? h->cap * 2 : 1 << 16;
        node *a = realloc(h->a, cap * sizeof(node));
        if (!a) return -1;
        h->a = a;
        h->cap = cap;
    }
    int64_t k = h->n++;
    node x = {f, g, s};
    while (k > 0) {
        int64_t p = (k - 1) / 2;
        if (!less(&x, &h->a[p])) break;
        h->a[k] = h->a[p];
        k = p;
    }
    h->a[k] = x;
    return 0;
}

static node pop(heap *h) {
    node top = h->a[0], x = h->a[--h->n];
    int64_t k = 0, n = h->n;
    for (;;) {
        int64_t c = 2 * k + 1;
        if (c >= n) break;
        if (c + 1 < n && less(&h->a[c + 1], &h->a[c])) c++;
        if (!less(&h->a[c], &x)) break;
        h->a[k] = h->a[c];
        k = c;
    }
    if (n > 0) h->a[k] = x;
    return top;
}

/* Returns the path length (cells written to out as L, j, i triples, source first), 0 if no path within max_exp
 * expansions, -1 if out is too small, -2 on allocation failure.  soft / via_soft may be NULL (no soft mode).
 * box receives the bounding box of the expanded cells (i_min, j_min, i_max, j_max).
 * axis (may be NULL) restricts F.Cu moves in escape zones: per cell 0 = free, 1 = vertical moves only, 2 = horizontal
 * only, 3 = either (zone corners); no diagonal move touches a restricted cell.
 * eband (may be NULL) is a per-cell flag: a step onto such a cell costs eband_cost extra, so the search keeps off
 * the board edge (the strip along the edge stays free for edge-mounted parts to escape).
 * fband (may be NULL) is the same for a chip's fan-out band: a track of a net with nothing to do with that chip
 * costs fband_cost there, so it cannot take the room the chip's via fan-out needs. */
int64_t astar(int nl, int ny, int nx,
              const uint8_t *flat, const uint8_t *soft, const uint8_t *hug,
              const uint8_t *via, const uint8_t *via_soft, const uint8_t *tgt,
              const double *layer_cost, const int32_t *src, int64_t n_src,
              int d0, int endd, int has_t, int tx, int ty,
              double hw, double via_cost, int max_turn, const double *turn_cost, double hug_cost,
              int64_t max_exp, int32_t *out, int64_t out_cap, int32_t *box, const uint8_t *axis,
              const uint8_t *eband, double eband_cost, const uint8_t *fband, double fband_cost)
{
    const int64_t plane = (int64_t)ny * nx, nstate = (int64_t)nl * plane * 9;
    double *g = calloc(nstate, sizeof(double));     /* untouched pages stay unallocated */
    uint8_t *prev = calloc(nstate, 1);              /* 0 unseen, 255 source, 1+d move from direction d, 16+L via */
    heap h = {0};
    int64_t result = 0, exp = 0;
    box[0] = nx; box[1] = ny; box[2] = -1; box[3] = -1;     /* i_min, j_min, i_max, j_max of expanded cells */
    if (!g || !prev) { result = -2; goto done; }

#define IDX(L, j, i, d) ((((int64_t)(L) * ny + (j)) * nx + (i)) * 9 + (d))
#define FREE(L, c) (flat[(int64_t)(L) * plane + (c)] || (soft && soft[(int64_t)(L) * plane + (c)]))
#define H(i, j) (has_t ? hw * ((double)(abs((i) - tx) + abs((j) - ty)) + \
                 (SQ2 - 2) * (double)(abs((i) - tx) < abs((j) - ty) ? abs((i) - tx) : abs((j) - ty))) : 0.0)

    for (int64_t k = 0; k < n_src; k++) {
        int L = src[3 * k], j = src[3 * k + 1], i = src[3 * k + 2];
        if (!FREE(L, (int64_t)j * nx + i)) continue;
        int64_t s = IDX(L, j, i, d0);
        g[s] = 0.0;
        prev[s] = 255;
        if (push(&h, H(i, j), 0.0, s)) { result = -2; goto done; }
    }
    while (h.n) {
        node top = pop(&h);
        int64_t s = top.s;
        double gc = top.g;
        if (g[s] < gc) continue;
        int d = (int)(s % 9), i = (int)((s / 9) % nx), j = (int)((s / 9 / nx) % ny), L = (int)(s / 9 / plane);
        int64_t c = (int64_t)j * nx + i;
        if (tgt[(int64_t)L * plane + c] && (endd < 0 || d == endd || d == 8)) {
            int64_t n = 0;
            for (;;) {
                if (n == out_cap) { result = -1; goto done; }
                out[3 * n] = L; out[3 * n + 1] = j; out[3 * n + 2] = i;
                n++;
                uint8_t p = prev[IDX(L, j, i, d)];
                if (p == 255) break;
                if (p >= 16) L = p - 16;
                else { i -= DI[d]; j -= DJ[d]; d = p - 1; }
            }
            for (int64_t a = 0, b = n - 1; a < b; a++, b--)
                for (int q = 0; q < 3; q++) { int32_t t = out[3 * a + q]; out[3 * a + q] = out[3 * b + q]; out[3 * b + q] = t; }
            result = n;
            goto done;
        }
        if (++exp > max_exp) break;
        if (i < box[0]) box[0] = i;
        if (j < box[1]) box[1] = j;
        if (i > box[2]) box[2] = i;
        if (j > box[3]) box[3] = j;
        double lc = layer_cost[L];
        for (int nd = 0; nd < 8; nd++) {
            double tcost = 0.0;
            if (d != 8) {
                int a = (nd - d + 8) % 8, b = (d - nd + 8) % 8, turn = a < b ? a : b;
                if (turn > max_turn) continue;
                tcost = turn_cost[turn];
            }
            int ni = i + DI[nd], nj = j + DJ[nd];
            if (ni < 0 || ni >= nx || nj < 0 || nj >= ny) continue;
            int64_t nc = (int64_t)nj * nx + ni;
            double pen = 0.0;
            if (!flat[(int64_t)L * plane + nc]) {
                if (soft && soft[(int64_t)L * plane + nc]) pen = 5.0;
                else continue;
            }
            int diag = DI[nd] && DJ[nd];
            if (diag && (!FREE(L, (int64_t)j * nx + ni) || !FREE(L, (int64_t)nj * nx + i))) continue;
            if (axis && L == 0) {
                uint8_t a1 = axis[c], a2 = axis[nc];
                if (a1 || a2) {
                    if (diag) continue;
                    uint8_t need = DI[nd] ? 2 : 1;              /* horizontal move needs bit 2, vertical bit 1 */
                    if ((a1 && !(a1 & need)) || (a2 && !(a2 & need))) continue;
                }
            }
            double step = (diag ? SQ2 : 1.0) * lc;
            if (hug[(int64_t)L * plane + nc]) step += hug_cost;
            if (eband && eband[nc]) step += eband_cost;
            if (fband && fband[nc]) step += fband_cost;
            double ng = gc + step + tcost + pen;
            int64_t ns = IDX(L, nj, ni, nd);
            if (!prev[ns] || ng < g[ns]) {
                g[ns] = ng;
                prev[ns] = (uint8_t)(1 + d);
                if (push(&h, ng + H(ni, nj), ng, ns)) { result = -2; goto done; }
            }
        }
        if (via[c] || (via_soft && via_soft[c])) {
            for (int OL = 0; OL < nl; OL++) {
                if (OL == L || !FREE(OL, c)) continue;
                double pen = via[c] ? 0.0 : 5.0;
                double ng = gc + via_cost + pen;
                int64_t ns = IDX(OL, j, i, d);
                if (!prev[ns] || ng < g[ns]) {
                    g[ns] = ng;
                    prev[ns] = (uint8_t)(16 + L);
                    if (push(&h, ng + H(i, j), ng, ns)) { result = -2; goto done; }
                }
            }
        }
    }
done:
    free(g);
    free(prev);
    free(h.a);
    return result;
}
