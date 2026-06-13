import { Navigate, Route, Routes } from 'react-router-dom';
import { DashboardPage } from '../pages/dashboard';
import { AuditPage } from '../pages/audit';
import { DenialsPage } from '../pages/denials';
import { VerifyPage } from '../pages/verify';
import { GrantsPage } from '../pages/grants';
import { RolesPage } from '../pages/roles';
import { BindingsPage } from '../pages/bindings';
import ConstraintsPage from '../pages/constraints';
import { ResourcesPage } from '../pages/resources';
import { IdentityPage } from '../pages/identity';
import { ModePage } from '../pages/mode';
import { SystemPage } from '../pages/system';

/**
 * Route table (设计系统 §5 / §6): one route per nav item (12 pages), each
 * wired to its implemented page component. Paths mirror the IA in `nav.ts`
 * (the single source of truth shared with the sidebar). An unknown path
 * fails closed to the dashboard (no blank screen).
 */
export function AppRoutes() {
  return (
    <Routes>
      <Route path="/" element={<DashboardPage />} />
      <Route path="/audit" element={<AuditPage />} />
      <Route path="/denials" element={<DenialsPage />} />
      <Route path="/verify" element={<VerifyPage />} />
      <Route path="/grants" element={<GrantsPage />} />
      <Route path="/roles" element={<RolesPage />} />
      <Route path="/bindings" element={<BindingsPage />} />
      <Route path="/constraints" element={<ConstraintsPage />} />
      <Route path="/resources" element={<ResourcesPage />} />
      <Route path="/principals" element={<IdentityPage />} />
      <Route path="/mode" element={<ModePage />} />
      <Route path="/system" element={<SystemPage />} />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
