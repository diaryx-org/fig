// ZON, as a library of its own: link libfig_zon.a beside libfig.a,
// and register it before reaching for a format of it.
//
//     int format;
//     FigError err = { .size = sizeof err };
//     if (fig_zon_register(&format, &err) != FIG_STATUS_OK) ...
//
// The dialects it registers, each found by name with fig_format_by_name:
// zon. Registering it again returns the format it already has.
#ifndef FIG_ZON_H
#define FIG_ZON_H

#include <fig.h>

#ifdef __cplusplus
extern "C" {
#endif

// Register ZON. On FIG_STATUS_OK, `*out_format` is the format of its
// first dialect; otherwise `out_err` (optional) says why.
FigStatus fig_zon_register(int *out_format, FigError *out_err);

#ifdef __cplusplus
}
#endif

#endif // FIG_ZON_H
