import { createContext, useContext } from 'react';
import type { Me } from './api';

export const MeContext = createContext<Me | null>(null);
export const useMe = () => useContext(MeContext);
export const useCanEdit = () => { const me = useMe(); return !me || me.role !== 'viewer'; };
/** `useCan('flags', 'write')` — mirrors the server's permission check for disabling controls with a hint. */
export const useCan = (resource: string, action: 'read' | 'write' | 'delete' = 'write') => {
  const me = useMe();
  if (!me?.permissions) return true;
  const has = (set?: ('read' | 'write' | 'delete')[]) => !!set && (set.includes(action) || (action === 'read' && (set.includes('write') || set.includes('delete'))));
  return resource in me.permissions ? has(me.permissions[resource]) : has(me.permissions['*']);
};
export const needs = (resource: string, action = 'write') => `needs ${resource}:${action}`;
