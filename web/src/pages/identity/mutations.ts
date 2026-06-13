/**
 * 主体/凭证页的写操作 hooks（本地）。
 *
 * 脚手架的 `api/hooks.ts` 只暴露了 principals/credentials 的读 hook 与端点函数，
 * 未提供这两类的写 mutation（仅 grants 有）。本页就地封装 `postPrincipal` /
 * `postCredential`，统一：成功后失效 principals + credentials 两个集合（凭证数
 * 联动）、把 409 冲突原样上抛给表单/确认层处理（ConflictError）。
 *
 * 写操作语义（与 10-principals-credentials.md §4 对齐）：
 *  - 新建主体：op=create。
 *  - 新建凭证：op=create（api_key 的一次性明文在响应里返回，UI 一次性展示）。
 *  - 吊销凭证：op=revoke + 期望 version（乐观锁；热生效、不可逆）。
 *  - 删除主体/凭证：op=delete + 期望 version（逻辑删除）。
 *
 * 所有 id/version 一律来自先前读取值，绝不臆造（§7 乐观锁纪律）。
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { endpoints, type WriteAck } from '../../api';

/** 新建主体请求体。*/
export interface PrincipalCreateBody {
  op: 'create';
  name: string;
  kind: string;
}

/** 删除主体请求体（逻辑删除，携期望 version）。*/
export interface PrincipalDeleteBody {
  op: 'delete';
  id: string;
  version: number;
}

/** 新建凭证请求体（secret 仅 token 携带，提交后即清；永不含 secret_hash）。*/
export interface CredentialCreateBody {
  op: 'create';
  principal_id: string;
  kind: string;
  trust_domain: string;
  /** 绝对过期时刻（ISO），null＝长期有效。*/
  expires_at: string | null;
  /** 仅 token：明文经控制面转发，本地不留存。其余 kind 省略。*/
  secret?: string;
}

/** 吊销凭证请求体（热生效、不可逆，携期望 version）。*/
export interface CredentialRevokeBody {
  op: 'revoke';
  id: string;
  version: number;
}

/** 删除凭证请求体（逻辑删除，携期望 version）。*/
export interface CredentialDeleteBody {
  op: 'delete';
  id: string;
  version: number;
}

/**
 * 新建凭证的成功响应：标准 WriteAck + api_key 创建时一次性返回的明文。
 * `api_key` 明文 **仅创建时一次性**出现于此，列表/详情永不回显。
 */
export interface CredentialCreateAck extends WriteAck {
  /** 仅 api_key 创建时存在；一次性展示后即不可得。*/
  api_key?: string;
}

function useInvalidateIdentity() {
  const qc = useQueryClient();
  return () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ['principals'] }),
      qc.invalidateQueries({ queryKey: ['credentials'] }),
    ]);
}

export function useCreatePrincipal() {
  const invalidate = useInvalidateIdentity();
  return useMutation({
    mutationFn: (body: PrincipalCreateBody) =>
      endpoints.postPrincipal(body) as Promise<WriteAck>,
    onSuccess: invalidate,
  });
}

export function useDeletePrincipal() {
  const invalidate = useInvalidateIdentity();
  return useMutation({
    mutationFn: (body: PrincipalDeleteBody) =>
      endpoints.postPrincipal(body) as Promise<WriteAck>,
    onSuccess: invalidate,
  });
}

export function useCreateCredential() {
  const invalidate = useInvalidateIdentity();
  return useMutation({
    mutationFn: (body: CredentialCreateBody) =>
      endpoints.postCredential(body) as Promise<CredentialCreateAck>,
    onSuccess: invalidate,
  });
}

export function useRevokeCredential() {
  const invalidate = useInvalidateIdentity();
  return useMutation({
    mutationFn: (body: CredentialRevokeBody) =>
      endpoints.postCredential(body) as Promise<WriteAck>,
    onSuccess: invalidate,
  });
}

export function useDeleteCredential() {
  const invalidate = useInvalidateIdentity();
  return useMutation({
    mutationFn: (body: CredentialDeleteBody) =>
      endpoints.postCredential(body) as Promise<WriteAck>,
    onSuccess: invalidate,
  });
}
