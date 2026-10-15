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
    private static native byte[] nativeRequest(byte[] root, byte[] request, int fd);
    static native byte[] nativePackages();

    static synchronized void attach(MainActivity value) { context=value.getApplicationContext(); activity=new WeakReference<>(value); }
    static synchronized void detach(MainActivity value) { if (activity.get()==value) activity.clear(); }
    public static String dataPath(String ignored) { return new File(context.getFilesDir(), "private").getAbsolutePath(); }
    public static String background(String ignored) { MainActivity a=activity.get();if(a!=null)main.post(()->a.moveTaskToBack(true));return ""; }
    static void requestNotificationPermission() {
        main.post(()->{
            MainActivity a=activity.get();
            if(a!=null && android.os.Build.VERSION.SDK_INT>=33 && a.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED)
                a.requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS},803);
        });
    }
    static String invoke(String request, int fd) {
        byte[] result=nativeRequest(dataPath("").getBytes(StandardCharsets.UTF_8), request.getBytes(StandardCharsets.UTF_8), fd);
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
            return result.get(45,TimeUnit.SECONDS);
        } catch(Exception e) {
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
