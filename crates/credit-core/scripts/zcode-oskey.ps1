param([string]$Target)
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public class __ZCK {
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] public struct C {
  public int Flags; public int Type; public IntPtr TargetName; public IntPtr Comment;
  public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
  public int CredentialBlobSize; public IntPtr CredentialBlob; public int Persist;
  public int AttributeCount; public IntPtr Attributes; public IntPtr TargetAlias; public IntPtr UserName; }
 [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)]
 public static extern bool CredRead(string t,int ty,int f,out IntPtr c);
}
'@
$p=[IntPtr]::Zero
if([__ZCK]::CredRead($Target,1,0,[ref]$p)){
  $c=[Runtime.InteropServices.Marshal]::PtrToStructure($p,[type][__ZCK+C])
  [Runtime.InteropServices.Marshal]::PtrToStringUni($c.CredentialBlob,[int]($c.CredentialBlobSize/2))
}