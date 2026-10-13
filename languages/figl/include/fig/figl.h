// figl, fig's authoring dialect, as a library of its own: link libfig_figl.a beside libfig.a,
// and register it before reaching for a format of it.
//
//     int format;
//     FigError err = { .size = sizeof err };
//     if (fig_figl_register(&format, &err) != FIG_STATUS_OK) ...
//
// The dialects it registers, each found by name with fig_format_by_name:
// figl. Registering it again returns the format it already has.
#ifndef FIG_FIGL_H
#define FIG_FIGL_H

#include <fig.h>

#ifdef __cplusplus
extern "C" {
#endif

// Register figl, fig's authoring dialect. On FIG_STATUS_OK, `*out_format` is the format of its
// first dialect; otherwise `out_err` (optional) says why.
FigStatus fig_figl_register(int *out_format, FigError *out_err);

#ifdef __cplusplus
}
#endif

#endif // FIG_FIGL_H
