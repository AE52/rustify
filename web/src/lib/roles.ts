import type { Role } from '../api/client'

/** Admins and owners may manage a team (TeamPolicy::manage*). */
export function isAdmin(role: string | null | undefined): boolean {
  return role === 'admin' || role === 'owner'
}

/** Selectable roles, highest-first (matches the server's `Role`). */
export const ROLE_OPTIONS: Role[] = ['owner', 'admin', 'member']

export function roleLabel(role: string): string {
  return role.charAt(0).toUpperCase() + role.slice(1)
}
