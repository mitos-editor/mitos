#include "plugin.h"
#include <stdlib.h>
#include <string.h>
// Tiny freestanding allocator for this generated-binding smoke fixture only.
extern unsigned char __heap_base;
typedef struct block { size_t size; struct block *next; bool available; } block;
static block *blocks;
static uintptr_t cursor;
static size_t aligned(size_t value) { return (value+15)&~(size_t)15; }
void *malloc(size_t size) {
  for (block *b=blocks;b;b=b->next) if (b->available && b->size>=size) { b->available=false;return (char *)b+aligned(sizeof(block)); }
  if (!cursor) cursor=aligned((uintptr_t)&__heap_base);
  size_t total=aligned(sizeof(block))+aligned(size);
  size_t wanted=(cursor+total+65535)/65536;
  size_t pages=__builtin_wasm_memory_size(0);
  if (wanted>pages && __builtin_wasm_memory_grow(0,wanted-pages)==(size_t)-1) return 0;
  block *b=(block *)cursor;cursor+=total;b->size=size;b->available=false;b->next=blocks;blocks=b;
  return (char *)b+aligned(sizeof(block));
}
void free(void *ptr) { if (ptr) ((block *)((char *)ptr-aligned(sizeof(block))))->available=true; }
void *realloc(void *ptr,size_t size) {
  if (!ptr) return malloc(size);
  block *b=(block *)((char *)ptr-aligned(sizeof(block)));
  if (b->size>=size) return ptr;
  void *next=malloc(size);if (!next) return 0;memcpy(next,ptr,b->size);free(ptr);return next;
}
_Noreturn void abort(void) { __builtin_trap(); }
void *memcpy(void *dst,const void *src,size_t size) { for(size_t i=0;i<size;i++) ((unsigned char *)dst)[i]=((const unsigned char *)src)[i];return dst; }
void *memset(void *dst,int value,size_t size) { for(size_t i=0;i<size;i++) ((unsigned char *)dst)[i]=(unsigned char)value;return dst; }
size_t strlen(const char *text) { size_t size=0;while(text[size])size++;return size; }
static bool is(plugin_string_t *text,const char *value) {size_t length=strlen(value);if (text->len!=length)return false;for(size_t i=0;i<length;i++)if(text->ptr[i]!=(uint8_t)value[i])return false;return true;}
bool exports_plugin_handle(plugin_request_t *request, plugin_failure_t *error) {
  mitos_plugin_host_own_effects_t owned;
  if (!mitos_plugin_host_begin_effects(&owned, error)) return false;
  mitos_plugin_host_borrow_effects_t effects=mitos_plugin_host_borrow_effects(owned);
  plugin_string_t text;
  bool success;
  if (request->command.is_some && is(&request->command.val,"read")) {
    success=mitos_plugin_host_read_document(request->editor.document.val.id,request->editor.document.val.version,0,6,&text,error);
    if (success) { success=mitos_plugin_host_method_effects_status(effects,&text,error);plugin_string_free(&text); }
  } else if (request->command.is_some && is(&request->command.val,"edit")) {
    mitos_plugin_host_own_edit_group_t edit;
    success=mitos_plugin_host_method_effects_edit(effects,request->editor.document.val.id,request->editor.document.val.version,&edit,error);
    if (success) {
      mitos_plugin_host_borrow_edit_group_t group=mitos_plugin_host_borrow_edit_group(edit);
      plugin_string_set(&text,"C");
      success=mitos_plugin_host_method_edit_group_add(group,0,6,&text,error)&&mitos_plugin_host_method_edit_group_finish(group,error);
      mitos_plugin_host_edit_group_drop_own(edit);
    }
  } else {
    plugin_string_set(&text,"c-ready");
    success=mitos_plugin_host_method_effects_status(effects,&text,error);
  }
  if (success) success=mitos_plugin_host_method_effects_finish(effects,error);
  mitos_plugin_host_effects_drop_own(owned);
  return success;
}
