package org.mukhomor.app;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.net.VpnService;
import android.os.Handler;
import android.os.Looper;
import org.json.JSONObject;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.ref.WeakReference;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

public final class NativeBridge {
    static boolean isP2P(String network){return "tor".equals(network)||"i2p".equals(network);}
    static { System.loadLibrary("mukhomor_core"); }
    static Context context;
    static volatile MukhomorVpnService service;
    private static WeakReference<MainActivity> activity = new WeakReference<>(null);
    private static final Handler main = new Handler(Looper.getMainLooper());
    private static final java.util.concurrent.ExecutorService documents=java.util.concurrent.Executors.newSingleThreadExecutor();
    private static CompletableFuture<Boolean> permission;
    private static CompletableFuture<String> selection;
    private static String selectionKind;
    private static Uri exportUri;
    static final AtomicLong generation = new AtomicLong();
    private static final java.util.concurrent.atomic.AtomicBoolean updateInstallerOpen=new java.util.concurrent.atomic.AtomicBoolean();
    static final int VPN_REQUEST = 801, FILE_REQUEST = 802;
    public static String language(String ignored) { return java.util.Locale.getDefault().toLanguageTag(); }
    public static String firstFrame(String ignored) {
        MainActivity a=activity.get();
        if(a!=null) main.post(a::firstFrame);
        return "";
    }
    public static String vpnSettings(String ignored) {
        main.post(()->{try{context.startActivity(new Intent(android.provider.Settings.ACTION_VPN_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));}catch(Exception ignoredError){}});
        return "";
    }
    public static String bridgeWebsite(String url) {
        if(!"https://bridges.torproject.org/".equals(url) && !"https://t.me/GetBridgesBot".equals(url)) return "Invalid bridge website";
        try {
            context.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            return "";
        } catch(Exception e) { return "Cannot open the bridge website"; }
    }
    private static native byte[] nativeRequest(byte[] root, byte[] request, byte[] torPath, int fd);
    static native byte[] nativePackages();

    static synchronized void attach(MainActivity value) { context=value.getApplicationContext(); activity=new WeakReference<>(value); }
    static synchronized void detach(MainActivity value) { if (activity.get()==value) activity.clear(); }
    public static String dataPath(String ignored) { return new File(context.getFilesDir(), "private").getAbsolutePath(); }
    public static String updateCache(String ignored) { return context.getCacheDir().getAbsolutePath(); }
    public static String prepareUpdate(String ignored) {
        if(updateInstallerOpen.get())return "ERROR:Подтвердите обновление в системном установщике";
        if(context.getPackageManager().canRequestPackageInstalls()) return "";
        MainActivity a=activity.get();
        if(a==null)return "ERROR:Откройте приложение для установки обновления";
        CompletableFuture<String> result=new CompletableFuture<>();
        main.post(()->{try {
            a.startActivity(new Intent(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,Uri.parse("package:"+context.getPackageName())));
            result.complete("ERROR:Разрешите обновления из Mukhomor и нажмите кнопку ещё раз");
        } catch(Exception e) { result.complete("ERROR:Не удалось открыть разрешение установки"); }});
        try { return result.get(30,TimeUnit.SECONDS); }
        catch(Exception e) { return "ERROR:Не удалось открыть разрешение установки"; }
    }
    public static String installUpdate(String path) {
        try {
            MainActivity a=activity.get();
            if(a==null) return "ERROR:Откройте приложение для установки обновления";
            File source=new File(path).getCanonicalFile();
            if(!source.getPath().startsWith(context.getCacheDir().getCanonicalPath()+File.separator))
                return "ERROR:Некорректный пакет обновления";
            android.content.pm.PackageManager pm=context.getPackageManager();
            android.content.pm.PackageInfo next=pm.getPackageArchiveInfo(source.getPath(),android.content.pm.PackageManager.GET_SIGNATURES);
            android.content.pm.PackageInfo current=pm.getPackageInfo(context.getPackageName(),android.content.pm.PackageManager.GET_SIGNATURES);
            if(next==null || !context.getPackageName().equals(next.packageName) || next.versionCode<=current.versionCode)
                return "ERROR:Некорректный пакет обновления";
            // Keep the existing signing identity. A debug install cannot be
            // silently replaced by a release APK; never suggest uninstalling.
            if(next.signatures==null || current.signatures==null || next.signatures.length!=1 || current.signatures.length!=1 || !next.signatures[0].equals(current.signatures[0]))
                return "ERROR:Подпись APK не совпадает с установленным приложением";
            CompletableFuture<String> result=new CompletableFuture<>();
            if(!pm.canRequestPackageInstalls()) {
                return prepareUpdate("");
            }
            File apk=new File(context.getCacheDir(),"verified-update.apk");
            try(InputStream in=new FileInputStream(source);FileOutputStream out=new FileOutputStream(apk)) {
                byte[] buffer=new byte[65536];int n;while((n=in.read(buffer))!=-1)out.write(buffer,0,n);out.getFD().sync();
            }
            main.post(()->{try {
                Uri uri=Uri.parse("content://"+context.getPackageName()+".updates/verified-update.apk");
                Intent intent=new Intent(Intent.ACTION_INSTALL_PACKAGE).setData(uri)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION).putExtra(Intent.EXTRA_RETURN_RESULT,true);
                a.startActivityForResult(intent,804);
                updateInstallerOpen.set(true);
                result.complete("");
            } catch(Exception e) { result.complete("ERROR:Не удалось запустить установщик обновления"); }});
            return result.get(30,TimeUnit.SECONDS);
        } catch(Exception e) { return "ERROR:Не удалось подготовить обновление Android"; }
    }
    public static String background(String ignored) { MainActivity a=activity.get();if(a!=null)main.post(()->a.moveTaskToBack(true));return ""; }
    static void requestNotificationPermission() {
        main.post(()->{
            MainActivity a=activity.get();
            if(a!=null && android.os.Build.VERSION.SDK_INT>=33 && a.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED)
                a.requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS},803);
        });
    }
    static String invoke(String request, int fd) {
        byte[] result=nativeRequest(dataPath("").getBytes(StandardCharsets.UTF_8), request.getBytes(StandardCharsets.UTF_8), new File(context.getApplicationInfo().nativeLibraryDir,"libmukhomor_tor.so").getAbsolutePath().getBytes(StandardCharsets.UTF_8), fd);
        if (result==null) return "{\"ok\":false,\"error\":\"Android native bridge failed\"}";
        return new String(result,StandardCharsets.UTF_8);
    }
    static String failure(String request, String message) {
        try {
            JSONObject response=new JSONObject(invoke("{\"action\":\"Status\"}",-1));
            response.put("ok",false); response.put("error",message);
            response.put("request_id",new JSONObject(request).optString("request_id"));
            return response.toString();
        } catch (Exception e) { return "{\"ok\":false,\"error\":\"Android operation failed\"}"; }
    }
    public static boolean protect(int fd) { MukhomorVpnService s=service; return s!=null && s.protect(fd); }

    public static String request(String request) {
        try {
            String action=new JSONObject(request).getString("action");
            if (action.equals("Disconnect")||action.equals("Exit")) {
                generation.incrementAndGet();
                synchronized(NativeBridge.class) { if(permission!=null) permission.complete(false); }
                String result=invoke(request,-1);
                MukhomorVpnService s=service; if(s!=null) s.stopVpn();
                return result;
            }
            if (!action.equals("Connect")) return invoke(request,-1);
            long ticket=generation.incrementAndGet();
            invoke("{\"action\":\"Status\"}",-1);
            Intent consent=VpnService.prepare(context);
            if (consent!=null) {
                MainActivity a=activity.get(); if(a==null) return failure(request,"Open Mukhomor to grant VPN permission");
                CompletableFuture<Boolean> pending=new CompletableFuture<>();
                synchronized(NativeBridge.class) { permission=pending; }
                main.post(()->a.startActivityForResult(consent,VPN_REQUEST));
                boolean allowed=pending.get(120,TimeUnit.SECONDS);
                synchronized(NativeBridge.class) { if(permission==pending) permission=null; }
                if(!allowed||ticket!=generation.get()) return failure(request,"VPN permission was denied or connection was cancelled");
            }
            if(ticket!=generation.get()) return failure(request,"Connection cancelled");
            CompletableFuture<String> result=MukhomorVpnService.enqueue(request,ticket);
            context.startForegroundService(new Intent(context,MukhomorVpnService.class).setAction("CONNECT"));
            return result.get(600,TimeUnit.SECONDS);
        } catch(Exception e) {
            MukhomorVpnService retained=service;
            if(retained!=null&&retained.holdsTorTunnel()){
                // An RPC timeout or notification error is not user consent to
                // release the blocking TUN. Explicit Disconnect still works.
                retained.reportTorFailure();
                return failure(request,"P2P operation failed; VPN tunnel retained until disconnect");
            }
            generation.incrementAndGet();
            invoke("{\"action\":\"Disconnect\"}",-1);
            MukhomorVpnService s=service;if(s!=null)s.stopVpn();
            return failure(request,"Android VPN could not start; reopen the app and retry");
        }
    }

    public static String chooseFile(String kind) {
        try {
            MainActivity a=activity.get();if(a==null)return "";
            CompletableFuture<String> pending=new CompletableFuture<>();
            synchronized(NativeBridge.class){if(selection!=null)return "";selection=pending;selectionKind=kind;}
            Intent intent=new Intent(kind.equals("save")?Intent.ACTION_CREATE_DOCUMENT:Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);intent.setType(kind.equals("profile")?"*/*":"application/json");
            if(kind.equals("save"))intent.putExtra(Intent.EXTRA_TITLE,"Mukhomor-preset.json");
            main.post(()->a.startActivityForResult(intent,FILE_REQUEST));
            return pending.get(180,TimeUnit.SECONDS);
        }catch(Exception e){return "";}finally{synchronized(NativeBridge.class){selection=null;}}
    }
    public static String finishExport(String path) {
        Uri uri; synchronized(NativeBridge.class){uri=exportUri;exportUri=null;}
        if(uri==null)return "";
        try(InputStream in=new FileInputStream(path);OutputStream out=context.getContentResolver().openOutputStream(uri,"wt")){
            if(out==null)throw new java.io.IOException();byte[] b=new byte[8192];int n;while((n=in.read(b))>=0)out.write(b,0,n);
            new File(path).delete();return "";
        }catch(Exception e){return "Cannot export the preset";}
    }
    static synchronized boolean activityResult(int code,int result,Intent data) {
        if(code==804){updateInstallerOpen.set(false);new File(context.getCacheDir(),"verified-update.apk").delete();return true;}
        if(code==VPN_REQUEST){if(permission!=null)permission.complete(result==Activity.RESULT_OK);return true;}
        if(code!=FILE_REQUEST)return false;
        if(selection==null)return true;
        if(result!=Activity.RESULT_OK||data==null||data.getData()==null){selection.complete("");return true;}
        Uri uri=data.getData();
        CompletableFuture<String> pending=selection;
        String kind=selectionKind;
        documents.execute(()->{try{
            File file=new File(context.getCacheDir(),"selected-"+java.util.UUID.randomUUID()+".json");
            if("save".equals(kind)){synchronized(NativeBridge.class){exportUri=uri;}pending.complete(file.getAbsolutePath());return;}
            int maximum="preset".equals(kind)?2*1024*1024:512*1024;
            try(InputStream in=context.getContentResolver().openInputStream(uri);OutputStream out=new FileOutputStream(file)){
                if(in==null)throw new java.io.IOException();byte[] buffer=new byte[8192];int n,total=0;
                while((n=in.read(buffer))>=0){total+=n;if(total>maximum)throw new java.io.IOException();out.write(buffer,0,n);}
            }catch(Exception e){file.delete();throw e;}
            if(!pending.complete(file.getAbsolutePath()))file.delete();
        }catch(Exception e){pending.complete("ERROR:Cannot read this document (UTF-8 profiles up to 512 KiB; presets up to 2 MiB)");}});
        return true;
    }
}
