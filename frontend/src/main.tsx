import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/brutal.css";
import { storeInit } from "./net/store";

// 先装载平台存储（桌面 ~/.goptop、移动端应用私有目录、鸿蒙 filesDir、Web localStorage）
// 再渲染：昵称/服务器/STUN/默认规则/头像在首帧就要可读，否则会先按空设置渲染一帧——
// 用户看到的是「设置丢了」。装载失败在门面内部已回退到浏览器存储，这里 finally 保证
// 无论成败都能出界面。
void storeInit().finally(() => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
});
