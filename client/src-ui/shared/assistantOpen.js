// 助理页里点到某个会话（事件卡片、去向条、活动列表）：在助理页右侧直接打开，不跳走（见 pages/Assistant.jsx）。
// 不在助理页时没有提供者，照旧跳到会话页
import { createContext, useContext } from 'react';

export const AssistantOpen = createContext(null);

/** 返回打开会话的函数：助理页里在右侧打开，别处跳到 /s/<id> */
export function useOpenSession(navigate) {
  const open = useContext(AssistantOpen);
  return (id) => (open ? open(id) : navigate(`/s/${id}`));
}
