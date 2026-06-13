/**
 * Roles write hook — POST /v1/roles (create / edit / logical-delete).
 *
 * There is no shared `useRole*` write hook in api/hooks.ts (only the read
 * `useRoles` + the `getRoles`/`postRole` endpoints). This page-local hook wraps
 * `postRole` and invalidates the roles collection on success. A 409 surfaces as
 * the shared `ConflictError` so the form can prompt "refresh & retry".
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { postRole } from '../../api/endpoints';
import type { Capability, GrantAction, WriteAck } from '../../api/types';

/** Body sent to POST /v1/roles. `version` is the optimistic-lock token read at
 * load time (absent on create). `delete_flag=1` is the logical-delete path. */
export interface RoleWriteBody {
  id?: string;
  name: string;
  description?: string;
  capabilities: { capability: Capability; action: GrantAction }[];
  inherits_from: string[];
  version?: number;
  delete_flag?: 0 | 1;
}

export function useRoleWrite() {
  const qc = useQueryClient();
  return useMutation<WriteAck, Error, RoleWriteBody>({
    mutationFn: (body) => postRole(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['roles'] }),
  });
}
