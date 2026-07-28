/* Denvion SplitSeal — seal-ffi C ABI (DSCP-2).
 * Every ss_* function returns a heap JSON C string owned by the caller;
 * release it with ss_free. */
#ifndef SEAL_FFI_H
#define SEAL_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

/* { "protocol":"DSCP-2", "version":2, "chain_id":7789 } */
char *ss_version(void);

/* StrictSeal: full seal -> (locked) -> L1 finalise -> (opened) transcript as JSON. */
char *ss_run_demo(void);

/* FastSeal: seal -> (locked) -> gateway pre-confirmation quorum -> (opened, pre-finality). */
char *ss_run_fast_demo(void);

/* Free a string returned by any ss_* function. Safe on NULL. */
void ss_free(char *ptr);

#ifdef __cplusplus
}
#endif

#endif /* SEAL_FFI_H */
