// 从别的页面（“文件”页的“发给对话”）把文字填进某个对话会话的输入框：先记下来再打开会话，
// 对话页挂上时（或已经挂着时收到 ll-chat-pending:{id}）取走填进去，不自动发送。
const pending = new Map();

export function sendToChat(id, text) {
  pending.set(id, text);
  window.dispatchEvent(new CustomEvent(`ll-chat-pending:${id}`));
}

/** 取走要填进 `id` 输入框的文字（没有时为空字符串）。 */
export function takeChatInsert(id) {
  const v = pending.get(id) ?? '';
  pending.delete(id);
  return v;
}
