/* A C client testing the public ABI of the Rust implementation. No C SQLite
 * engine is compiled into this client. Run the same client against the oracle. */
#include "sqlite3.h"
#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int destroyed, traced, logged;
static void log_callback(void *context, int code, const char *message) {
  assert(context == &logged); assert(message);
  if(logged == 0) { assert(code == SQLITE_NOTICE); assert(strcmp(message,"test 42 Rust") == 0); }
  logged++;
}
static void check(int rc) { if (rc != SQLITE_OK) { fprintf(stderr,"rc=%d\n",rc); abort(); } }
static void exec(sqlite3 *db, const char *sql) {
  char *message = 0;
  int rc = sqlite3_exec(db, sql, 0, 0, &message);
  if (rc != SQLITE_OK) { fprintf(stderr,"%s: %s\n",sql,message); sqlite3_free(message); abort(); }
}
static sqlite3_int64 scalar(sqlite3 *db, const char *sql) {
  sqlite3_stmt *s = 0; check(sqlite3_prepare_v2(db, sql, -1, &s, 0));
  assert(sqlite3_step(s) == SQLITE_ROW);
  sqlite3_int64 value = sqlite3_column_int64(s, 0);
  check(sqlite3_finalize(s)); return value;
}
static void destroy(void *p) { assert(p == &destroyed); destroyed++; }
static void twice(sqlite3_context *ctx, int argc, sqlite3_value **values) {
  assert(argc == 1); assert(sqlite3_user_data(ctx) == &destroyed);
  sqlite3_result_int64(ctx, sqlite3_value_int64(values[0]) * 2);
}
static int trace(unsigned event, void *context, void *statement, void *sql) {
  (void)statement; (void)sql; assert(context == &traced);
  if(event == SQLITE_TRACE_STMT) traced++;
  return 0;
}
static int conflict(void *context, int reason, sqlite3_changeset_iter *iter) {
  (void)context; (void)reason; (void)iter; return SQLITE_CHANGESET_ABORT;
}
static void formatted(char *out, int size, const char *fmt, ...) {
  va_list args; va_start(args, fmt); sqlite3_vsnprintf(size, out, fmt, args); va_end(args);
}
static int progress(void *p) { (*(int*)p)++; return 1; }
static int authorizer(void *p, int action, const char *a, const char *b, const char *c, const char *d) {
  (void)p; (void)a; (void)b; (void)c; (void)d;
  return action == SQLITE_DELETE ? SQLITE_DENY : SQLITE_OK;
}

int main(void) {
  check(sqlite3_shutdown());
  check(sqlite3_config(SQLITE_CONFIG_LOG, log_callback, &logged));
  sqlite3_log(SQLITE_NOTICE, "test %d %s", 42, "Rust");
  assert(logged == 1);
  sqlite3 *db = 0; check(sqlite3_open(":memory:", &db));
  assert(strcmp(sqlite3_libversion(), "3.53.4") == 0);
  assert(sqlite3_threadsafe() == 1);
  assert(sqlite3_vfs_find(0) != 0);
  check(sqlite3_create_function_v2(db, "twice", 1, SQLITE_UTF8|SQLITE_DETERMINISTIC,
    &destroyed, twice, 0, 0, destroy));
  check(sqlite3_trace_v2(db, SQLITE_TRACE_STMT, trace, &traced));
  assert(scalar(db, "SELECT twice(21)") == 42);
  assert(traced > 0);

  char text[100]; formatted(text, sizeof(text), "%s:%d:%lld", "Rust", 42, (long long)1234567890123);
  assert(strcmp(text, "Rust:42:1234567890123") == 0);
  char *escaped = sqlite3_mprintf("%Q:%04d", "it's", 7);
  assert(strcmp(escaped, "'it''s':0007") == 0); sqlite3_free(escaped);

  exec(db, "CREATE TABLE t(id INTEGER PRIMARY KEY, data BLOB); INSERT INTO t VALUES(1,zeroblob(8))");
  sqlite3_blob *blob = 0; check(sqlite3_blob_open(db,"main","t","data",1,1,&blob));
  const unsigned char bytes[] = {0,255,127,128}; unsigned char readback[4] = {0};
  assert(sqlite3_blob_bytes(blob) == 8);
  check(sqlite3_blob_write(blob,bytes,4,2)); check(sqlite3_blob_read(blob,readback,4,2));
  assert(memcmp(bytes,readback,4) == 0); check(sqlite3_blob_close(blob));

  sqlite3 *copy = 0; check(sqlite3_open(":memory:",&copy));
  sqlite3_backup *backup = sqlite3_backup_init(copy,"main",db,"main"); assert(backup);
  assert(sqlite3_backup_step(backup,-1) == SQLITE_DONE); check(sqlite3_backup_finish(backup));
  assert(scalar(copy,"SELECT length(data) FROM t") == 8);
  check(sqlite3_close(copy));

  sqlite3_int64 size = 0; unsigned char *serialized = sqlite3_serialize(db,"main",&size,0);
  assert(serialized && size > 0); check(sqlite3_open(":memory:",&copy));
  check(sqlite3_deserialize(copy,"main",serialized,size,size,SQLITE_DESERIALIZE_FREEONCLOSE));
  assert(scalar(copy,"SELECT id FROM t") == 1); check(sqlite3_close(copy));

  exec(db,"CREATE TABLE changes(id INTEGER PRIMARY KEY,value TEXT)");
  sqlite3_session *session = 0; check(sqlite3session_create(db,"main",&session));
  check(sqlite3session_attach(session,"changes"));
  exec(db,"INSERT INTO changes VALUES(1,'one'),(2,'two'); UPDATE changes SET value='updated' WHERE id=1");
  int nchangeset = 0; void *changeset = 0; check(sqlite3session_changeset(session,&nchangeset,&changeset));
  assert(nchangeset > 0); check(sqlite3_open(":memory:",&copy));
  exec(copy,"CREATE TABLE changes(id INTEGER PRIMARY KEY,value TEXT)");
  check(sqlite3changeset_apply(copy,nchangeset,changeset,0,conflict,0));
  assert(scalar(copy,"SELECT count(*) FROM changes") == 2);
  assert(scalar(copy,"SELECT value='updated' FROM changes WHERE id=1") == 1);
  sqlite3_free(changeset); sqlite3session_delete(session); check(sqlite3_close(copy));

  sqlite3_stmt *s = 0;
  check(sqlite3_prepare_v3(db,"SELECT 123, 'abc'",-1,SQLITE_PREPARE_NORMALIZE,&s,0));
  assert(strstr(sqlite3_normalized_sql(s),"?") != 0); check(sqlite3_finalize(s));
  const unsigned short sql16[] = {'S','E','L','E','C','T',' ','4','2',0};
  check(sqlite3_prepare16_v2(db,sql16,-1,&s,0));
  assert(sqlite3_step(s)==SQLITE_ROW); assert(sqlite3_column_int(s,0)==42); check(sqlite3_finalize(s));

  int nprogress=0; sqlite3_progress_handler(db,10,progress,&nprogress);
  assert(sqlite3_exec(db,"WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n",0,0,0)==SQLITE_INTERRUPT);
  assert(nprogress>0); sqlite3_progress_handler(db,0,0,0);
  check(sqlite3_set_authorizer(db,authorizer,0));
  assert(sqlite3_exec(db,"DELETE FROM t",0,0,0)==SQLITE_AUTH);
  check(sqlite3_set_authorizer(db,0,0));
  check(sqlite3_close(db)); assert(destroyed==1);
  check(sqlite3_shutdown());
  assert(sqlite3_memory_used()==0);
  puts("PASS: C ABI, callbacks, variadics, blobs, backup, serialization, sessions, UTF-16, normalization, cancellation, authorization, cleanup");
  return 0;
}
