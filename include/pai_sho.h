/* C entry for an app that links libpai_sho.a and runs the process in the app.
   One process. The socket is opened so other tools can talk to it.
   The iOS app does not have to use the socket. */

#pragma once

#include <stdint.h>

/* Returns 0 when the background thread is spawned.
   -1 when host or socket_path is missing, or host is not an IP.
   -2 when a process is already running.
   key_path may be NULL. */
int32_t pai_sho_start(const char *host, const char *socket_path, const char *key_path);

/* Asks that process to stop. */
void pai_sho_stop(void);

/* 1 while a process started by pai_sho_start is running, else 0. */
int32_t pai_sho_running(void);
