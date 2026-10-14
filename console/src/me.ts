import { createContext, useContext } from 'react';
import type { Me } from './api';

export const MeContext = createContext<Me | null>(null);
export const useMe = () => useContext(MeContext);
export const useCanEdit = () => { const me = useMe(); return !me || me.role !== 'viewer'; };
