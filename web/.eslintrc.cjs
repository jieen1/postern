/* eslint-env node */
module.exports = {
  root: true,
  env: { browser: true, es2021: true, node: true },
  parser: '@typescript-eslint/parser',
  parserOptions: { ecmaVersion: 'latest', sourceType: 'module' },
  plugins: ['@typescript-eslint', 'react-hooks', 'react-refresh'],
  extends: [
    'eslint:recommended',
    'plugin:@typescript-eslint/recommended',
    'plugin:react-hooks/recommended',
  ],
  ignorePatterns: [
    'dist',
    'node_modules',
    'playwright-report',
    'test-results',
    '*.config.ts',
    '*.config.js',
    '.eslintrc.cjs',
  ],
  rules: {
    // Fast-refresh dev hint only; several components deliberately co-locate a
    // small pure helper (e.g. StageChip + stageOrder), which is sound design,
    // so this hint is disabled rather than fragmenting modules for HMR.
    'react-refresh/only-export-components': 'off',
    '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
    '@typescript-eslint/no-explicit-any': 'error',
  },
};
