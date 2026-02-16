#ifndef IBUS_HANDY_WRAPPER_H
#define IBUS_HANDY_WRAPPER_H

#include <ibus.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef gboolean (*ibus_handy_callback_key_event)(void* ctx, IBusEngine* engine, guint keyval, guint keycode, guint modifiers);
typedef void (*ibus_handy_callback_focus_in)(void* ctx, IBusEngine* engine);
typedef void (*ibus_handy_callback_focus_out)(void* ctx, IBusEngine* engine);
typedef void (*ibus_handy_callback_reset)(void* ctx, IBusEngine* engine);
typedef void (*ibus_handy_callback_enable)(void* ctx, IBusEngine* engine);
typedef void (*ibus_handy_callback_disable)(void* ctx, IBusEngine* engine);

void ibus_handy_set_callback(
    void* ctx,
    ibus_handy_callback_key_event key_event_cb,
    ibus_handy_callback_focus_in focus_in_cb,
    ibus_handy_callback_focus_out focus_out_cb,
    ibus_handy_callback_reset reset_cb,
    ibus_handy_callback_enable enable_cb,
    ibus_handy_callback_disable disable_cb
);

void ibus_handy_init(bool ibus_mode);

typedef struct {
    IBusEngine parent;
} IBusHandyEngine;

#ifdef __cplusplus
}
#endif

#endif
