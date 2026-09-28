/// <reference types="vite/client" />

// Vite 的资源查询后缀（?url / ?raw / ?worker）在这里声明，供 pdf.js worker 使用。
declare module "*?url" {
  const src: string;
  export default src;
}
